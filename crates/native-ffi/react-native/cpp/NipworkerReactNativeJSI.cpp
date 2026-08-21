#include "NipworkerReactNativeJSI.h"

#include <ReactCommon/CallInvoker.h>
#include <jsi/jsi.h>

#include <algorithm>
#include <atomic>
#include <cstring>
#include <limits>
#include <mutex>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

extern "C" {
#include "nipworker.h"
}

namespace nipworker::react_native {
namespace {

using facebook::jsi::Array;
using facebook::jsi::ArrayBuffer;
using facebook::jsi::Function;
using facebook::jsi::MutableBuffer;
using facebook::jsi::Object;
using facebook::jsi::PropNameID;
using facebook::jsi::Runtime;
using facebook::jsi::Value;

constexpr char kRuntimeName[] = "__nipworkerReactNativeByteRuntime";
constexpr char kWakeHandlerName[] = "__wakeHandler";
constexpr char kGenerationName[] = "__generation";
constexpr std::uint8_t kRouteMagic[] = {'N', 'W', 'R', '1'};

std::atomic<Generation> gNextRuntimeGeneration{1};

void releaseRustPacket(std::uint8_t* data, std::size_t size) noexcept {
	nipworker_free_bytes(data, size);
}

bool parseRoute(const OwnedPacket& packet, std::string& route) {
	if (packet.size() < 8 || std::memcmp(packet.data(), kRouteMagic, sizeof(kRouteMagic)) != 0) {
		return false;
	}
	std::uint32_t routeSize = 0;
	std::memcpy(&routeSize, packet.data() + 4, sizeof(routeSize));
#if __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
	routeSize = __builtin_bswap32(routeSize);
#endif
	if (routeSize == 0 || static_cast<std::size_t>(routeSize) != packet.size() - 8) return false;
	route.assign(reinterpret_cast<const char*>(packet.data() + 8), routeSize);
	return true;
}

bool isArrayBuffer(Runtime& runtime, const Value& value) {
	return value.isObject() && value.asObject(runtime).isArrayBuffer(runtime);
}

class RustMutableBuffer final : public MutableBuffer {
public:
	explicit RustMutableBuffer(OwnedPacket packet) : packet_(std::move(packet)) {}
	std::size_t size() const override { return packet_.size(); }
	std::uint8_t* data() override { return packet_.data(); }

private:
	OwnedPacket packet_;
};

class SubscriptionPin final {
public:
	explicit SubscriptionPin(void* token) : token_(token) {}
	~SubscriptionPin() { release(); }
	void release() noexcept {
		if (!released_.exchange(true, std::memory_order_acq_rel) && token_ != nullptr) {
			nipworker_subscription_pin_release(token_);
		}
	}

private:
	void* token_;
	std::atomic_bool released_{false};
};

class SubscriptionMutableBuffer final : public MutableBuffer {
public:
	SubscriptionMutableBuffer(
		std::shared_ptr<SubscriptionPin> pin,
		std::uint8_t* data,
		std::size_t size
	) : pin_(std::move(pin)), data_(data), size_(size) {}
	std::size_t size() const override { return size_; }
	std::uint8_t* data() override { return data_; }

private:
	std::shared_ptr<SubscriptionPin> pin_;
	std::uint8_t* data_;
	std::size_t size_;
};

} // namespace

struct EngineHost::Impl {
	struct CallbackContext {
		EngineHost* host;
		Generation engineGeneration;
	};

	mutable std::mutex mutex;
	void* handle = nullptr;
	Generation engineGeneration = 0;
	std::weak_ptr<RuntimeTransport> activeTransport;
	Generation activeRuntimeGeneration = 0;
	std::vector<std::unique_ptr<CallbackContext>> callbackContexts;
};

void EngineHost::callback(void* userdata, const std::uint8_t* bytes, std::size_t size) {
	auto* context = static_cast<EngineHost::Impl::CallbackContext*>(userdata);
	OwnedPacket packet(const_cast<std::uint8_t*>(bytes), size, releaseRustPacket);
	if (context == nullptr || bytes == nullptr || size == 0) return;

	std::shared_ptr<RuntimeTransport> transport;
	Generation activeEngineGeneration = 0;
	{
		auto& impl = *context->host->impl_;
		std::lock_guard<std::mutex> lock(impl.mutex);
		activeEngineGeneration = impl.engineGeneration;
		if (context->engineGeneration == activeEngineGeneration) {
			transport = impl.activeTransport.lock();
		}
	}
	if (transport) {
		transport->acceptEnginePacket(
			context->engineGeneration,
			activeEngineGeneration,
			std::move(packet)
		);
	}
}

EngineHost::EngineHost() : impl_(std::make_unique<Impl>()) {}

EngineHost& EngineHost::shared() {
	static EngineHost host;
	return host;
}

void* EngineHost::configure(
	const std::string& storagePath,
	const std::string& defaultRelays,
	const std::string& indexerRelays,
	bool meshEnabled
) {
	std::lock_guard<std::mutex> lock(impl_->mutex);
	if (impl_->handle != nullptr) return impl_->handle;
	impl_->engineGeneration++;
	if (impl_->engineGeneration == 0) impl_->engineGeneration++;
	auto context = std::make_unique<Impl::CallbackContext>();
	context->host = this;
	context->engineGeneration = impl_->engineGeneration;
	auto* userdata = context.get();
	impl_->callbackContexts.emplace_back(std::move(context));
	impl_->handle = nipworker_init_with_options(
		callback,
		userdata,
		storagePath.empty() ? nullptr : storagePath.c_str(),
		defaultRelays.empty() ? nullptr : defaultRelays.c_str(),
		indexerRelays.empty() ? nullptr : indexerRelays.c_str(),
		meshEnabled
	);
	return impl_->handle;
}

void* EngineHost::handle() const {
	std::lock_guard<std::mutex> lock(impl_->mutex);
	return impl_->handle;
}

void EngineHost::deinit() {
	void* handle = nullptr;
	Generation retiredGeneration = 0;
	{
		std::lock_guard<std::mutex> lock(impl_->mutex);
		handle = impl_->handle;
		retiredGeneration = impl_->engineGeneration;
		impl_->handle = nullptr;
		impl_->engineGeneration++;
		impl_->activeTransport.reset();
		impl_->activeRuntimeGeneration = 0;
	}
	if (handle != nullptr) nipworker_deinit(handle);
	// nipworker_deinit synchronously joins the engine thread, so no callback can
	// still reference this generation. Preserve any newer context installed by a
	// concurrent configure while reclaiming only the retired generation.
	if (handle != nullptr) {
		std::lock_guard<std::mutex> lock(impl_->mutex);
		impl_->callbackContexts.erase(
			std::remove_if(
				impl_->callbackContexts.begin(),
				impl_->callbackContexts.end(),
				[retiredGeneration](const auto& context) {
					return context->engineGeneration == retiredGeneration;
				}
			),
			impl_->callbackContexts.end()
		);
	}
}

void EngineHost::bind(const std::shared_ptr<RuntimeTransport>& transport) {
	std::lock_guard<std::mutex> lock(impl_->mutex);
	impl_->activeTransport = transport;
	impl_->activeRuntimeGeneration = transport ? transport->generation() : 0;
}

void EngineHost::unbind(Generation runtimeGeneration) {
	std::lock_guard<std::mutex> lock(impl_->mutex);
	if (impl_->activeRuntimeGeneration != runtimeGeneration) return;
	impl_->activeTransport.reset();
	impl_->activeRuntimeGeneration = 0;
}

class RuntimeTransport::Impl {
public:
	Impl(
		Generation generation,
		std::shared_ptr<facebook::react::CallInvoker> callInvoker,
		DeliveryLimits limits
	) : state(std::make_shared<DeliveryState>(generation, limits)),
		callInvoker(std::move(callInvoker)), limits(limits) {}

	std::shared_ptr<DeliveryState> state;
	std::mutex mutex;
	std::shared_ptr<facebook::react::CallInvoker> callInvoker;
	DeliveryLimits limits;
	std::unordered_set<std::string> registeredRoutes;
	std::size_t registeredRouteBytes = 0;
	std::unordered_map<std::string, std::size_t> logicalLeases;
};

std::shared_ptr<RuntimeTransport> RuntimeTransport::create(
	std::shared_ptr<facebook::react::CallInvoker> callInvoker,
	DeliveryLimits limits
) {
	auto generation = gNextRuntimeGeneration.fetch_add(1, std::memory_order_relaxed);
	if (generation == 0) generation = gNextRuntimeGeneration.fetch_add(1, std::memory_order_relaxed);
	auto transport = std::shared_ptr<RuntimeTransport>(
		new RuntimeTransport(generation, std::move(callInvoker), limits)
	);
	transport->initializeScheduler();
	return transport;
}

RuntimeTransport::RuntimeTransport(
	Generation generation,
	std::shared_ptr<facebook::react::CallInvoker> callInvoker,
	DeliveryLimits limits
) : generation_(generation), impl_(std::make_unique<Impl>(generation, std::move(callInvoker), limits)) {}

RuntimeTransport::~RuntimeTransport() {
	invalidate();
}

void RuntimeTransport::initializeScheduler() {
	// Scheduling is enabled only after JS installs a handler. This lets packets
	// queue safely during module/bootstrap ordering without an empty-handler loop.
}

void RuntimeTransport::setHandlerInstalled(bool installed) {
	if (!installed) {
		impl_->state->setSchedule({});
		return;
	}
	std::weak_ptr<RuntimeTransport> weak = shared_from_this();
	impl_->state->setSchedule([weak](Generation generation) {
		if (auto transport = weak.lock()) transport->scheduleWake(generation);
	});
}

void RuntimeTransport::scheduleWake(Generation generation) {
	std::shared_ptr<facebook::react::CallInvoker> invoker;
	{
		std::lock_guard<std::mutex> lock(impl_->mutex);
		invoker = impl_->callInvoker;
	}
	if (!invoker || !impl_->state->alive() || generation != generation_) {
		impl_->state->finishWake(generation);
		return;
	}
	std::weak_ptr<RuntimeTransport> weak = shared_from_this();
	invoker->invokeAsync([weak, generation](Runtime& runtime) {
		if (auto transport = weak.lock()) transport->runWake(runtime, generation);
	});
}

void RuntimeTransport::runWake(Runtime& runtime, Generation generation) {
	if (!impl_->state->alive() || generation != generation_) return;
	try {
		Value runtimeValue = runtime.global().getProperty(runtime, kRuntimeName);
		if (runtimeValue.isObject()) {
			Object byteRuntime = runtimeValue.asObject(runtime);
			Value installedGeneration = byteRuntime.getProperty(runtime, kGenerationName);
			Value handler = byteRuntime.getProperty(runtime, kWakeHandlerName);
			if (installedGeneration.isNumber() &&
				static_cast<Generation>(installedGeneration.asNumber()) == generation_ &&
				handler.isObject() && handler.asObject(runtime).isFunction(runtime)) {
				handler.asObject(runtime).asFunction(runtime).call(runtime);
			}
		}
	} catch (...) {
		// Delivery state must always clear/recheck even when the JS handler throws.
	}
	impl_->state->finishWake(generation);
}

void RuntimeTransport::acceptEnginePacket(
	Generation engineGeneration,
	Generation activeEngineGeneration,
	OwnedPacket packet
) {
	if (engineGeneration != activeEngineGeneration) {
		impl_->state->enqueueControl(generation_ + 1, std::move(packet));
		return;
	}
	std::string route;
	if (parseRoute(packet, route)) {
		impl_->state->enqueueRoute(generation_, std::move(route));
		return;
	}
	impl_->state->enqueueControl(generation_, std::move(packet));
}

DeliveryStats RuntimeTransport::stats() const {
	return impl_->state->stats();
}

bool RuntimeTransport::alive() const {
	return impl_->state->alive();
}

void RuntimeTransport::invalidate() {
	if (!impl_->state->alive()) return;
	EngineHost::shared().unbind(generation_);
	impl_->state->invalidate(generation_);
	std::unordered_map<std::string, std::size_t> logicalLeases;
	{
		std::lock_guard<std::mutex> lock(impl_->mutex);
		logicalLeases.swap(impl_->logicalLeases);
		impl_->callInvoker.reset();
		impl_->registeredRoutes.clear();
		impl_->registeredRouteBytes = 0;
	}
	if (auto* handle = EngineHost::shared().handle()) {
		for (const auto& [route, count] : logicalLeases) {
			for (std::size_t index = 0; index < count; index++) {
				nipworker_release_subscription(handle, route.c_str());
			}
		}
		nipworker_cleanup_subscriptions(handle);
	}
}

void RuntimeTransport::install(Runtime& runtime) {
	EngineHost::shared().bind(shared_from_this());
	Object byteRuntime(runtime);
	byteRuntime.setProperty(runtime, kGenerationName, static_cast<double>(generation_));

	auto weak = std::weak_ptr<RuntimeTransport>(shared_from_this());
	auto add = [&](const char* name, unsigned int count, Function function) {
		byteRuntime.setProperty(runtime, name, std::move(function));
	};

	add("init", 0, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "init"), 0,
		[](Runtime&, const Value&, const Value*, std::size_t) { return Value::undefined(); }
	));

	add("setWakeHandler", 1, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "setWakeHandler"), 1,
		[weak](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			auto transport = weak.lock();
			if (!transport) return Value::undefined();
			Object byteRuntime = runtime.global().getPropertyAsObject(runtime, kRuntimeName);
			const bool installed = count > 0 && args[0].isObject() &&
				args[0].asObject(runtime).isFunction(runtime);
			byteRuntime.setProperty(
				runtime,
				kWakeHandlerName,
				installed ? Value(runtime, args[0]) : Value::undefined()
			);
			transport->setHandlerInstalled(installed);
			return Value::undefined();
		}
	));

	add("drainPending", 0, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "drainPending"), 0,
		[weak](Runtime& runtime, const Value&, const Value*, std::size_t) {
			Object result(runtime);
			auto transport = weak.lock();
			if (!transport) {
				result.setProperty(runtime, "routes", Array(runtime, 0));
				result.setProperty(runtime, "packets", Array(runtime, 0));
				return Value(runtime, std::move(result));
			}
			auto batch = transport->impl_->state->drain(transport->generation_);
			Array routes(runtime, batch.routes.size());
			for (std::size_t index = 0; index < batch.routes.size(); index++) {
				routes.setValueAtIndex(runtime, index, facebook::jsi::String::createFromUtf8(runtime, batch.routes[index]));
			}
			Array packets(runtime, batch.controls.size());
			for (std::size_t index = 0; index < batch.controls.size(); index++) {
				auto storage = std::make_shared<RustMutableBuffer>(std::move(batch.controls[index]));
				packets.setValueAtIndex(runtime, index, ArrayBuffer(runtime, std::move(storage)));
			}
			result.setProperty(runtime, "routes", std::move(routes));
			result.setProperty(runtime, "packets", std::move(packets));
			return Value(runtime, std::move(result));
		}
	));

	add("getDeliveryStats", 0, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "getDeliveryStats"), 0,
		[weak](Runtime& runtime, const Value&, const Value*, std::size_t) {
			Object result(runtime);
			auto transport = weak.lock();
			if (!transport) return Value(runtime, std::move(result));
			auto stats = transport->stats();
#define NIPWORKER_STAT(name) result.setProperty(runtime, #name, static_cast<double>(stats.name))
			NIPWORKER_STAT(receivedRoutes);
			NIPWORKER_STAT(receivedControls);
			NIPWORKER_STAT(coalescedRoutes);
			NIPWORKER_STAT(scheduledWakes);
			NIPWORKER_STAT(executedWakes);
			NIPWORKER_STAT(droppedControlPackets);
			NIPWORKER_STAT(droppedControlBytes);
			NIPWORKER_STAT(droppedRoutes);
			NIPWORKER_STAT(staleDrops);
			NIPWORKER_STAT(invalidatedDrops);
			NIPWORKER_STAT(queuedControlPackets);
			NIPWORKER_STAT(queuedControlBytes);
			NIPWORKER_STAT(dirtyRoutes);
			NIPWORKER_STAT(dirtyRouteBytes);
			NIPWORKER_STAT(dirtyRouteBytesHighWater);
			NIPWORKER_STAT(controlBytesHighWater);
#undef NIPWORKER_STAT
			return Value(runtime, std::move(result));
		}
	));

	auto engineHandle = []() { return EngineHost::shared().handle(); };
	add("handleMessage", 1, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "handleMessage"), 1,
		[weak, engineHandle](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 1 || !isArrayBuffer(runtime, args[0])) return Value::undefined();
			auto buffer = args[0].asObject(runtime).getArrayBuffer(runtime);
			if (auto* handle = engineHandle()) nipworker_handle_message(handle, buffer.data(runtime), buffer.size(runtime));
			return Value::undefined();
		}
	));

	add("setPrivateKey", 1, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "setPrivateKey"), 1,
		[weak, engineHandle](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 1 || !args[0].isString()) return Value::undefined();
			auto secret = args[0].asString(runtime).utf8(runtime);
			if (auto* handle = engineHandle()) nipworker_set_private_key(handle, secret.c_str());
			return Value::undefined();
		}
	));

	auto addNoArgEngineCall = [&](const char* name, void (*call)(void*)) {
		add(name, 0, Function::createFromHostFunction(
			runtime, PropNameID::forAscii(runtime, name), 0,
			[weak, engineHandle, call](Runtime&, const Value&, const Value*, std::size_t) {
				if (weak.lock()) if (auto* handle = engineHandle()) call(handle);
				return Value::undefined();
			}
		));
	};
	addNoArgEngineCall("wake", nipworker_wake);
	addNoArgEngineCall("clearSigner", nipworker_clear_signer);
	addNoArgEngineCall("removeSigner", nipworker_remove_signer);
	add("cleanupSubscriptions", 0, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "cleanupSubscriptions"), 0,
		[weak, engineHandle](Runtime&, const Value&, const Value*, std::size_t) {
			if (auto transport = weak.lock()) {
				if (auto* handle = engineHandle()) {
					nipworker_cleanup_subscriptions(handle);
					// A memory pin owns its backing independently and does not keep the
					// logical route alive. After native cleanup, zero-lease routes cannot
					// emit and their admission budget can safely be reclaimed.
					std::lock_guard<std::mutex> lock(transport->impl_->mutex);
					for (auto route = transport->impl_->registeredRoutes.begin();
						route != transport->impl_->registeredRoutes.end();) {
						if (transport->impl_->logicalLeases.find(*route) ==
							transport->impl_->logicalLeases.end()) {
							transport->impl_->registeredRouteBytes -= route->size();
							route = transport->impl_->registeredRoutes.erase(route);
						} else {
							++route;
						}
					}
				}
			}
			return Value::undefined();
		}
	));

	auto reserveRoute = [weak](const std::string& route) -> std::pair<bool, bool> {
		auto transport = weak.lock();
		if (!transport || route.empty()) return {false, false};
		std::lock_guard<std::mutex> lock(transport->impl_->mutex);
		if (!transport->impl_->state->alive()) return {false, false};
		auto& registered = transport->impl_->registeredRoutes;
		if (registered.find(route) != registered.end()) return {true, false};
		const auto& limits = transport->impl_->limits;
		const auto routeBytes = route.size();
		if (routeBytes > limits.maxRouteBytes || routeBytes > limits.maxDirtyRouteBytes ||
			registered.size() >= limits.maxDirtyRoutes ||
			transport->impl_->registeredRouteBytes > limits.maxDirtyRouteBytes - routeBytes) {
			return {false, false};
		}
		registered.insert(route);
		transport->impl_->registeredRouteBytes += routeBytes;
		return {true, true};
	};
	auto rollbackRoute = [weak](const std::string& route, bool inserted) {
		if (!inserted) return;
		if (auto transport = weak.lock()) {
			std::lock_guard<std::mutex> lock(transport->impl_->mutex);
			if (transport->impl_->registeredRoutes.erase(route) != 0) {
				transport->impl_->registeredRouteBytes -= route.size();
			}
		}
	};
	auto recordLease = [weak](const std::string& route) {
		if (auto transport = weak.lock()) {
			std::lock_guard<std::mutex> lock(transport->impl_->mutex);
			if (transport->impl_->state->alive()) transport->impl_->logicalLeases[route]++;
		}
	};
	auto consumeLease = [weak](const std::string& route) -> bool {
		auto transport = weak.lock();
		if (!transport) return false;
		std::lock_guard<std::mutex> lock(transport->impl_->mutex);
		if (!transport->impl_->state->alive()) return false;
		auto found = transport->impl_->logicalLeases.find(route);
		if (found == transport->impl_->logicalLeases.end() || found->second == 0) return false;
		if (--found->second == 0) transport->impl_->logicalLeases.erase(found);
		return true;
	};

	auto createPinnedBuffer = [weak](Runtime& runtime, const std::string& route) -> Value {
		auto transport = weak.lock();
		if (!transport || route.empty()) return Value::undefined();
		std::shared_ptr<SubscriptionPin> pin;
		std::uint8_t* data = nullptr;
		std::size_t size = 0;
		{
			// Serialize token acquisition with invalidate. The opaque Rust token,
			// not the engine handle, owns storage after this critical section.
			std::lock_guard<std::mutex> lock(transport->impl_->mutex);
			if (!transport->impl_->state->alive()) return Value::undefined();
			auto* handle = EngineHost::shared().handle();
			if (handle == nullptr) return Value::undefined();
			auto* token = nipworker_subscription_pin(handle, route.c_str(), &data, &size);
			if (token == nullptr || data == nullptr || size == 0) {
				if (token != nullptr) nipworker_subscription_pin_release(token);
				return Value::undefined();
			}
			pin = std::make_shared<SubscriptionPin>(token);
		}
		auto storage = std::make_shared<SubscriptionMutableBuffer>(pin, data, size);
		return Value(runtime, ArrayBuffer(runtime, std::move(storage)));
	};

	add("registerSubscription", 2, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "registerSubscription"), 2,
		[weak, engineHandle, reserveRoute, rollbackRoute, recordLease](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 2 || !args[0].isString() || !args[1].isNumber()) return Value(false);
			auto route = args[0].asString(runtime).utf8(runtime);
			auto size = args[1].asNumber();
			if (size <= 0 || size > static_cast<double>(std::numeric_limits<std::size_t>::max())) return Value(false);
			auto [reserved, inserted] = reserveRoute(route);
			if (!reserved) return Value(false);
			auto* handle = engineHandle();
			const bool registered = handle && nipworker_register_subscription(handle, route.c_str(), static_cast<std::size_t>(size));
			if (!registered) rollbackRoute(route, inserted);
			if (registered) recordLease(route);
			return Value(registered);
		}
	));

	add("registerPublishBuffer", 2, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "registerPublishBuffer"), 2,
		[weak, engineHandle, reserveRoute, rollbackRoute, recordLease](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 2 || !args[0].isString() || !args[1].isNumber()) return Value(false);
			auto route = args[0].asString(runtime).utf8(runtime);
			auto size = args[1].asNumber();
			if (size <= 0 || size > static_cast<double>(std::numeric_limits<std::size_t>::max())) return Value(false);
			auto [reserved, inserted] = reserveRoute(route);
			if (!reserved) return Value(false);
			auto* handle = engineHandle();
			const bool registered = handle && nipworker_register_publish_buffer(handle, route.c_str(), static_cast<std::size_t>(size));
			if (!registered) rollbackRoute(route, inserted);
			if (registered) recordLease(route);
			return Value(registered);
		}
	));

	add("subscribe", 2, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "subscribe"), 2,
		[weak, engineHandle, reserveRoute, rollbackRoute, recordLease, createPinnedBuffer](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 2 || !isArrayBuffer(runtime, args[0]) || !args[1].isString()) return Value::undefined();
			auto message = args[0].asObject(runtime).getArrayBuffer(runtime);
			auto route = args[1].asString(runtime).utf8(runtime);
			auto [reserved, inserted] = reserveRoute(route);
			if (!reserved) return Value::undefined();
			auto* handle = engineHandle();
			if (!handle || !nipworker_subscribe_message(handle, message.data(runtime), message.size(runtime))) {
				rollbackRoute(route, inserted);
				return Value::undefined();
			}
			auto result = createPinnedBuffer(runtime, route);
			if (result.isUndefined()) {
				nipworker_release_subscription(handle, route.c_str());
				rollbackRoute(route, inserted);
			} else {
				recordLease(route);
			}
			return result;
		}
	));

	add("publish", 2, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "publish"), 2,
		[weak, engineHandle, reserveRoute, rollbackRoute, recordLease, createPinnedBuffer](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 2 || !isArrayBuffer(runtime, args[0]) || !args[1].isString()) return Value::undefined();
			auto message = args[0].asObject(runtime).getArrayBuffer(runtime);
			auto route = args[1].asString(runtime).utf8(runtime);
			auto [reserved, inserted] = reserveRoute(route);
			if (!reserved) return Value::undefined();
			auto* handle = engineHandle();
			if (!handle || !nipworker_publish_message(handle, message.data(runtime), message.size(runtime))) {
				rollbackRoute(route, inserted);
				return Value::undefined();
			}
			auto result = createPinnedBuffer(runtime, route);
			if (result.isUndefined()) {
				nipworker_release_subscription(handle, route.c_str());
				rollbackRoute(route, inserted);
			} else {
				recordLease(route);
			}
			return result;
		}
	));

	add("retainSubscription", 1, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "retainSubscription"), 1,
		[weak, engineHandle, recordLease](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 1 || !args[0].isString()) return Value(false);
			auto route = args[0].asString(runtime).utf8(runtime);
			auto* handle = engineHandle();
			const bool retained = handle && nipworker_retain_subscription(handle, route.c_str());
			if (retained) recordLease(route);
			return Value(retained);
		}
	));

	add("retainSubscriptionBuffer", 1, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "retainSubscriptionBuffer"), 1,
		[weak, engineHandle, recordLease, createPinnedBuffer](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 1 || !args[0].isString()) return Value::undefined();
			auto route = args[0].asString(runtime).utf8(runtime);
			auto* handle = engineHandle();
			if (!handle || !nipworker_retain_subscription(handle, route.c_str())) return Value::undefined();
			auto result = createPinnedBuffer(runtime, route);
			if (result.isUndefined()) nipworker_release_subscription(handle, route.c_str());
			else recordLease(route);
			return result;
		}
	));

	add("getSubscriptionBuffer", 1, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "getSubscriptionBuffer"), 1,
		[weak, createPinnedBuffer](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (!weak.lock() || count < 1 || !args[0].isString()) return Value::undefined();
			return createPinnedBuffer(runtime, args[0].asString(runtime).utf8(runtime));
		}
	));

	add("releaseSubscription", 1, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "releaseSubscription"), 1,
		[weak, engineHandle, consumeLease](Runtime& runtime, const Value&, const Value* args, std::size_t count) {
			if (weak.lock() && count > 0 && args[0].isString()) {
				auto route = args[0].asString(runtime).utf8(runtime);
				if (consumeLease(route)) {
					if (auto* handle = engineHandle()) nipworker_release_subscription(handle, route.c_str());
				}
			}
			return Value::undefined();
		}
	));

	add("invalidate", 0, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "invalidate"), 0,
		[weak](Runtime&, const Value&, const Value*, std::size_t) {
			if (auto transport = weak.lock()) transport->invalidate();
			return Value::undefined();
		}
	));

	add("deinit", 0, Function::createFromHostFunction(
		runtime, PropNameID::forAscii(runtime, "deinit"), 0,
		[weak](Runtime&, const Value&, const Value*, std::size_t) {
			if (auto transport = weak.lock()) transport->invalidate();
			EngineHost::shared().deinit();
			return Value::undefined();
		}
	));

	runtime.global().setProperty(runtime, kRuntimeName, std::move(byteRuntime));
}

} // namespace nipworker::react_native
