#pragma once

#include "NipworkerReactNativeTransport.h"

#include <memory>
#include <string>

namespace facebook::jsi {
class Runtime;
}

namespace facebook::react {
class CallInvoker;
}

namespace nipworker::react_native {

class RuntimeTransport;

class EngineHost final {
public:
	static EngineHost& shared();

	void* configure(
		const std::string& storagePath,
		const std::string& defaultRelays,
		const std::string& indexerRelays,
		bool meshEnabled
	);
	void* handle() const;
	void deinit();
	void shutdownProcess();
	void bind(const std::shared_ptr<RuntimeTransport>& transport);
	void unbind(Generation runtimeGeneration);

	EngineHost(const EngineHost&) = delete;
	EngineHost& operator=(const EngineHost&) = delete;

private:
	EngineHost();
	~EngineHost() = default;
	static void callback(void* userdata, const std::uint8_t* bytes, std::size_t size);
	struct Impl;
	std::unique_ptr<Impl> impl_;
};

class RuntimeTransport final : public std::enable_shared_from_this<RuntimeTransport> {
public:
	static std::shared_ptr<RuntimeTransport> create(
		std::shared_ptr<facebook::react::CallInvoker> callInvoker,
		DeliveryLimits limits = {}
	);
	~RuntimeTransport();

	bool install(facebook::jsi::Runtime& runtime);
	void acceptEnginePacket(Generation engineGeneration, Generation activeEngineGeneration, OwnedPacket packet);
	void invalidate();
	Generation generation() const noexcept { return generation_; }
	DeliveryStats stats() const;
	bool alive() const;

	RuntimeTransport(const RuntimeTransport&) = delete;
	RuntimeTransport& operator=(const RuntimeTransport&) = delete;

private:
	class Impl;
	RuntimeTransport(
		Generation generation,
		std::shared_ptr<facebook::react::CallInvoker> callInvoker,
		DeliveryLimits limits
	);
	void initializeScheduler();
	void setHandlerInstalled(bool installed);
	void scheduleWake(Generation generation);
	void runWake(facebook::jsi::Runtime& runtime, Generation generation);

	const Generation generation_;
	std::unique_ptr<Impl> impl_;
};

} // namespace nipworker::react_native
