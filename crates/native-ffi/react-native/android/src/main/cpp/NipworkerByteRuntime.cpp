#include <jni.h>
#include <jsi/jsi.h>

#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <utility>
#include <vector>

extern "C" void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
extern "C" bool nipworker_subscribe_message(void* handle, const uint8_t* ptr, size_t len);
extern "C" bool nipworker_publish_message(void* handle, const uint8_t* ptr, size_t len);
extern "C" void nipworker_set_private_key(void* handle, const char* ptr);
extern "C" void nipworker_wake(void* handle);
extern "C" void nipworker_deinit(void* handle);
extern "C" bool nipworker_register_subscription(void* handle, const char* sub_id, size_t buffer_size);
extern "C" bool nipworker_register_publish_buffer(void* handle, const char* publish_id, size_t buffer_size);
extern "C" bool nipworker_retain_subscription(void* handle, const char* sub_id);
extern "C" void nipworker_release_subscription(void* handle, const char* sub_id);
extern "C" uint8_t* nipworker_subscription_buffer_ptr(void* handle, const char* sub_id);
extern "C" size_t nipworker_subscription_buffer_len(void* handle, const char* sub_id);
extern "C" void nipworker_cleanup_subscriptions(void* handle);

namespace {
using facebook::jsi::Array;
using facebook::jsi::ArrayBuffer;
using facebook::jsi::Function;
using facebook::jsi::MutableBuffer;
using facebook::jsi::Object;
using facebook::jsi::PropNameID;
using facebook::jsi::Runtime;
using facebook::jsi::String;
using facebook::jsi::Value;

class VectorMutableBuffer final : public MutableBuffer {
public:
	explicit VectorMutableBuffer(std::vector<uint8_t> bytes) : bytes_(std::move(bytes)) {}

	size_t size() const override {
		return bytes_.size();
	}

	uint8_t* data() override {
		return bytes_.data();
	}

private:
	std::vector<uint8_t> bytes_;
};

class ExternalMutableBuffer final : public MutableBuffer {
public:
	ExternalMutableBuffer(void* engineHandle, std::string subId, uint8_t* ptr, size_t len)
		: engineHandle_(engineHandle), subId_(std::move(subId)), ptr_(ptr), len_(len) {}

	~ExternalMutableBuffer() override {
		if (engineHandle_ != nullptr && !subId_.empty()) {
			nipworker_release_subscription(engineHandle_, subId_.c_str());
		}
	}

	size_t size() const override {
		return len_;
	}

	uint8_t* data() override {
		return ptr_;
	}

private:
	void* engineHandle_;
	std::string subId_;
	uint8_t* ptr_;
	size_t len_;
};

std::shared_ptr<ExternalMutableBuffer> createPinnedBuffer(
	void* engineHandle,
	const std::string& subId,
	uint8_t* ptr,
	size_t len
) {
	if (engineHandle == nullptr || subId.empty() || ptr == nullptr || len == 0 ||
		!nipworker_retain_subscription(engineHandle, subId.c_str())) {
		return nullptr;
	}
	return std::make_shared<ExternalMutableBuffer>(engineHandle, subId, ptr, len);
}

std::mutex gQueueMutex;
std::vector<std::vector<uint8_t>> gQueuedPackets;
bool gInstalled = false;

void enqueueBytes(const uint8_t* ptr, size_t len) {
	if (ptr == nullptr || len == 0) return;
	std::vector<uint8_t> packet(len);
	std::memcpy(packet.data(), ptr, len);
	std::lock_guard<std::mutex> lock(gQueueMutex);
	gQueuedPackets.emplace_back(std::move(packet));
}

std::vector<std::vector<uint8_t>> drainBytes() {
	std::lock_guard<std::mutex> lock(gQueueMutex);
	std::vector<std::vector<uint8_t>> packets;
	packets.swap(gQueuedPackets);
	return packets;
}

bool isArrayBuffer(Runtime& runtime, const Value& value) {
	return value.isObject() && value.asObject(runtime).isArrayBuffer(runtime);
}
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeQueueData(
	JNIEnv* env,
	jclass,
	jbyteArray bytes
) {
	if (bytes == nullptr) return JNI_FALSE;
	jsize len = env->GetArrayLength(bytes);
	jbyte* ptr = env->GetByteArrayElements(bytes, nullptr);
	if (ptr == nullptr) return JNI_FALSE;
	enqueueBytes(reinterpret_cast<const uint8_t*>(ptr), static_cast<size_t>(len));
	env->ReleaseByteArrayElements(bytes, ptr, JNI_ABORT);
	return JNI_TRUE;
}

extern "C" JNIEXPORT jobject JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeSubscribeMessage(
	JNIEnv* env,
	jclass,
	jlong handle,
	jbyteArray bytes,
	jstring subId
) {
	if (handle == 0 || bytes == nullptr || subId == nullptr) return nullptr;
	jsize len = env->GetArrayLength(bytes);
	jbyte* ptr = env->GetByteArrayElements(bytes, nullptr);
	if (ptr == nullptr) return nullptr;
	auto* engineHandle = reinterpret_cast<void*>(handle);
	bool ok = nipworker_subscribe_message(
		engineHandle,
		reinterpret_cast<const uint8_t*>(ptr),
		static_cast<size_t>(len)
	);
	env->ReleaseByteArrayElements(bytes, ptr, JNI_ABORT);
	if (!ok) return nullptr;

	const char* subIdChars = env->GetStringUTFChars(subId, nullptr);
	if (subIdChars == nullptr) return nullptr;
	uint8_t* bufferPtr = nipworker_subscription_buffer_ptr(engineHandle, subIdChars);
	size_t bufferLen = nipworker_subscription_buffer_len(engineHandle, subIdChars);
	env->ReleaseStringUTFChars(subId, subIdChars);
	if (bufferPtr == nullptr || bufferLen == 0) return nullptr;
	return env->NewDirectByteBuffer(bufferPtr, static_cast<jlong>(bufferLen));
}

extern "C" JNIEXPORT jobject JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativePublishMessage(
	JNIEnv* env,
	jclass,
	jlong handle,
	jbyteArray bytes,
	jstring publishId
) {
	if (handle == 0 || bytes == nullptr || publishId == nullptr) return nullptr;
	jsize len = env->GetArrayLength(bytes);
	jbyte* ptr = env->GetByteArrayElements(bytes, nullptr);
	if (ptr == nullptr) return nullptr;
	auto* engineHandle = reinterpret_cast<void*>(handle);
	bool ok = nipworker_publish_message(
		engineHandle,
		reinterpret_cast<const uint8_t*>(ptr),
		static_cast<size_t>(len)
	);
	env->ReleaseByteArrayElements(bytes, ptr, JNI_ABORT);
	if (!ok) return nullptr;

	const char* publishIdChars = env->GetStringUTFChars(publishId, nullptr);
	if (publishIdChars == nullptr) return nullptr;
	uint8_t* bufferPtr = nipworker_subscription_buffer_ptr(engineHandle, publishIdChars);
	size_t bufferLen = nipworker_subscription_buffer_len(engineHandle, publishIdChars);
	env->ReleaseStringUTFChars(publishId, publishIdChars);
	if (bufferPtr == nullptr || bufferLen == 0) return nullptr;
	return env->NewDirectByteBuffer(bufferPtr, static_cast<jlong>(bufferLen));
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeRegisterSubscription(
	JNIEnv* env,
	jclass,
	jlong handle,
	jstring subId,
	jint bufferSize
) {
	if (handle == 0 || subId == nullptr || bufferSize <= 0) return JNI_FALSE;
	const char* subIdChars = env->GetStringUTFChars(subId, nullptr);
	if (subIdChars == nullptr) return JNI_FALSE;
	bool ok = nipworker_register_subscription(
		reinterpret_cast<void*>(handle),
		subIdChars,
		static_cast<size_t>(bufferSize)
	);
	env->ReleaseStringUTFChars(subId, subIdChars);
	return ok ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeRegisterPublishBuffer(
	JNIEnv* env,
	jclass,
	jlong handle,
	jstring publishId,
	jint bufferSize
) {
	if (handle == 0 || publishId == nullptr || bufferSize <= 0) return JNI_FALSE;
	const char* publishIdChars = env->GetStringUTFChars(publishId, nullptr);
	if (publishIdChars == nullptr) return JNI_FALSE;
	bool ok = nipworker_register_publish_buffer(
		reinterpret_cast<void*>(handle),
		publishIdChars,
		static_cast<size_t>(bufferSize)
	);
	env->ReleaseStringUTFChars(publishId, publishIdChars);
	return ok ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeRetainSubscription(
	JNIEnv* env,
	jclass,
	jlong handle,
	jstring subId
) {
	if (handle == 0 || subId == nullptr) return JNI_FALSE;
	const char* subIdChars = env->GetStringUTFChars(subId, nullptr);
	if (subIdChars == nullptr) return JNI_FALSE;
	bool ok = nipworker_retain_subscription(reinterpret_cast<void*>(handle), subIdChars);
	env->ReleaseStringUTFChars(subId, subIdChars);
	return ok ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT void JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeReleaseSubscription(
	JNIEnv* env,
	jclass,
	jlong handle,
	jstring subId
) {
	if (handle == 0 || subId == nullptr) return;
	const char* subIdChars = env->GetStringUTFChars(subId, nullptr);
	if (subIdChars == nullptr) return;
	nipworker_release_subscription(reinterpret_cast<void*>(handle), subIdChars);
	env->ReleaseStringUTFChars(subId, subIdChars);
}

extern "C" JNIEXPORT jobject JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeGetSubscriptionBuffer(
	JNIEnv* env,
	jclass,
	jlong handle,
	jstring subId
) {
	if (handle == 0 || subId == nullptr) return nullptr;
	const char* subIdChars = env->GetStringUTFChars(subId, nullptr);
	if (subIdChars == nullptr) return nullptr;
	auto* engineHandle = reinterpret_cast<void*>(handle);
	uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, subIdChars);
	size_t len = nipworker_subscription_buffer_len(engineHandle, subIdChars);
	env->ReleaseStringUTFChars(subId, subIdChars);
	if (ptr == nullptr || len == 0) return nullptr;
	return env->NewDirectByteBuffer(ptr, static_cast<jlong>(len));
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeIsByteRuntimeInstalled(
	JNIEnv*,
	jclass
) {
	return gInstalled ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeInstallByteRuntime(
	JNIEnv*,
	jclass,
	jlong runtimePtr,
	jlong handle
) {
	if (runtimePtr == 0 || handle == 0) return JNI_FALSE;
	auto& runtime = *reinterpret_cast<Runtime*>(runtimePtr);
	auto* engineHandle = reinterpret_cast<void*>(handle);

	Object byteRuntime(runtime);

	byteRuntime.setProperty(
		runtime,
		"init",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "init"),
			0,
			[](Runtime& runtime, const Value&, const Value*, size_t) -> Value {
				return Value::undefined();
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"handleMessage",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "handleMessage"),
			1,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 1 || !isArrayBuffer(runtime, args[0])) return Value::undefined();
				ArrayBuffer buffer = args[0].asObject(runtime).getArrayBuffer(runtime);
				nipworker_handle_message(engineHandle, buffer.data(runtime), buffer.size(runtime));
				return Value::undefined();
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"setPrivateKey",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "setPrivateKey"),
			1,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 1 || !args[0].isString()) return Value::undefined();
				std::string secret = args[0].asString(runtime).utf8(runtime);
				nipworker_set_private_key(engineHandle, secret.c_str());
				return Value::undefined();
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"wake",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "wake"),
			0,
			[engineHandle](Runtime&, const Value&, const Value*, size_t) -> Value {
				nipworker_wake(engineHandle);
				return Value::undefined();
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"registerSubscription",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "registerSubscription"),
			2,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 2 || !args[0].isString() || !args[1].isNumber()) return Value(false);
				std::string subId = args[0].asString(runtime).utf8(runtime);
				auto bufferSize = static_cast<size_t>(args[1].asNumber());
				return Value(nipworker_register_subscription(engineHandle, subId.c_str(), bufferSize));
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"subscribe",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "subscribe"),
			2,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 2 || !isArrayBuffer(runtime, args[0]) || !args[1].isString()) {
					return Value::undefined();
				}
				ArrayBuffer message = args[0].asObject(runtime).getArrayBuffer(runtime);
				std::string subId = args[1].asString(runtime).utf8(runtime);
				if (!nipworker_subscribe_message(engineHandle, message.data(runtime), message.size(runtime))) {
					return Value::undefined();
				}
				uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, subId.c_str());
				size_t len = nipworker_subscription_buffer_len(engineHandle, subId.c_str());
				if (ptr == nullptr || len == 0) return Value::undefined();
				auto nativeBuffer = createPinnedBuffer(engineHandle, subId, ptr, len);
				if (!nativeBuffer) {
					nipworker_release_subscription(engineHandle, subId.c_str());
					return Value::undefined();
				}
				ArrayBuffer buffer(runtime, std::move(nativeBuffer));
				return Value(runtime, std::move(buffer));
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"registerPublishBuffer",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "registerPublishBuffer"),
			2,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 2 || !args[0].isString() || !args[1].isNumber()) return Value(false);
				std::string publishId = args[0].asString(runtime).utf8(runtime);
				auto bufferSize = static_cast<size_t>(args[1].asNumber());
				return Value(nipworker_register_publish_buffer(engineHandle, publishId.c_str(), bufferSize));
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"publish",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "publish"),
			2,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 2 || !isArrayBuffer(runtime, args[0]) || !args[1].isString()) {
					return Value::undefined();
				}
				ArrayBuffer message = args[0].asObject(runtime).getArrayBuffer(runtime);
				std::string publishId = args[1].asString(runtime).utf8(runtime);
				if (!nipworker_publish_message(engineHandle, message.data(runtime), message.size(runtime))) {
					return Value::undefined();
				}
				uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, publishId.c_str());
				size_t len = nipworker_subscription_buffer_len(engineHandle, publishId.c_str());
				if (ptr == nullptr || len == 0) return Value::undefined();
				auto nativeBuffer = createPinnedBuffer(engineHandle, publishId, ptr, len);
				if (!nativeBuffer) {
					nipworker_release_subscription(engineHandle, publishId.c_str());
					return Value::undefined();
				}
				ArrayBuffer buffer(runtime, std::move(nativeBuffer));
				return Value(runtime, std::move(buffer));
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"retainSubscription",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "retainSubscription"),
			1,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 1 || !args[0].isString()) return Value(false);
				std::string subId = args[0].asString(runtime).utf8(runtime);
				return Value(nipworker_retain_subscription(engineHandle, subId.c_str()));
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"retainSubscriptionBuffer",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "retainSubscriptionBuffer"),
			1,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 1 || !args[0].isString()) return Value::undefined();
				std::string subId = args[0].asString(runtime).utf8(runtime);
				if (!nipworker_retain_subscription(engineHandle, subId.c_str())) {
					return Value::undefined();
				}
				uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, subId.c_str());
				size_t len = nipworker_subscription_buffer_len(engineHandle, subId.c_str());
				if (ptr == nullptr || len == 0) {
					nipworker_release_subscription(engineHandle, subId.c_str());
					return Value::undefined();
				}
				auto nativeBuffer = createPinnedBuffer(engineHandle, subId, ptr, len);
				if (!nativeBuffer) {
					nipworker_release_subscription(engineHandle, subId.c_str());
					return Value::undefined();
				}
				ArrayBuffer buffer(runtime, std::move(nativeBuffer));
				return Value(runtime, std::move(buffer));
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"releaseSubscription",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "releaseSubscription"),
			1,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 1 || !args[0].isString()) return Value::undefined();
				std::string subId = args[0].asString(runtime).utf8(runtime);
				nipworker_release_subscription(engineHandle, subId.c_str());
				return Value::undefined();
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"getSubscriptionBuffer",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "getSubscriptionBuffer"),
			1,
			[engineHandle](Runtime& runtime, const Value&, const Value* args, size_t count) -> Value {
				if (count < 1 || !args[0].isString()) return Value::undefined();
				std::string subId = args[0].asString(runtime).utf8(runtime);
				uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, subId.c_str());
				size_t len = nipworker_subscription_buffer_len(engineHandle, subId.c_str());
				if (ptr == nullptr || len == 0) return Value::undefined();
				auto nativeBuffer = createPinnedBuffer(engineHandle, subId, ptr, len);
				if (!nativeBuffer) return Value::undefined();
				ArrayBuffer buffer(runtime, std::move(nativeBuffer));
				return Value(runtime, std::move(buffer));
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"cleanupSubscriptions",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "cleanupSubscriptions"),
			0,
			[engineHandle](Runtime&, const Value&, const Value*, size_t) -> Value {
				nipworker_cleanup_subscriptions(engineHandle);
				return Value::undefined();
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"deinit",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "deinit"),
			0,
			[engineHandle](Runtime&, const Value&, const Value*, size_t) -> Value {
				nipworker_deinit(engineHandle);
				return Value::undefined();
			}
		)
	);

	byteRuntime.setProperty(
		runtime,
		"drain",
		Function::createFromHostFunction(
			runtime,
			PropNameID::forAscii(runtime, "drain"),
			0,
			[](Runtime& runtime, const Value&, const Value*, size_t) -> Value {
				auto packets = drainBytes();
				Array output(runtime, packets.size());
				for (size_t i = 0; i < packets.size(); i++) {
					auto nativeBuffer = std::make_shared<VectorMutableBuffer>(std::move(packets[i]));
					ArrayBuffer buffer(runtime, std::move(nativeBuffer));
					output.setValueAtIndex(runtime, i, std::move(buffer));
				}
				return output;
			}
		)
	);

	runtime.global().setProperty(
		runtime,
		"__nipworkerReactNativeByteRuntime",
		std::move(byteRuntime)
	);
	gInstalled = true;
	return JNI_TRUE;
}
