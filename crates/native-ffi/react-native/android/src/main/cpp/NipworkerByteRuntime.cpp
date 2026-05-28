#include <jni.h>
#include <jsi/jsi.h>

#include <cstdint>
#include <cstring>
#include <mutex>
#include <string>
#include <vector>

extern "C" void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
extern "C" void nipworker_set_private_key(void* handle, const char* ptr);
extern "C" void nipworker_deinit(void* handle);

namespace {
using facebook::jsi::Array;
using facebook::jsi::ArrayBuffer;
using facebook::jsi::Function;
using facebook::jsi::Object;
using facebook::jsi::PropNameID;
using facebook::jsi::Runtime;
using facebook::jsi::String;
using facebook::jsi::Value;

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
					ArrayBuffer buffer(runtime, packets[i].size());
					std::memcpy(buffer.data(runtime), packets[i].data(), packets[i].size());
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
