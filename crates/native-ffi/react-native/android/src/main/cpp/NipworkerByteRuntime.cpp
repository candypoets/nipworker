#include <jni.h>

#include <ReactCommon/CallInvokerHolder.h>
#include <fbjni/fbjni.h>
#include <jsi/jsi.h>

#include "../../../../cpp/NipworkerReactNativeJSI.h"

#include <memory>
#include <string>

using nipworker::react_native::EngineHost;
using nipworker::react_native::RuntimeTransport;

namespace {

std::string fromJString(JNIEnv* env, jstring value) {
	if (value == nullptr) return {};
	const char* chars = env->GetStringUTFChars(value, nullptr);
	if (chars == nullptr) return {};
	std::string output(chars);
	env->ReleaseStringUTFChars(value, chars);
	return output;
}

std::shared_ptr<RuntimeTransport>* transportBox(jlong token) {
	return reinterpret_cast<std::shared_ptr<RuntimeTransport>*>(token);
}

} // namespace

extern "C" JNIEXPORT jlong JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeInstallTransport(
	JNIEnv*,
	jclass,
	jlong runtimePointer,
	jobject callInvokerHolder
) {
	if (runtimePointer == 0 || callInvokerHolder == nullptr) return 0;
	using Holder = facebook::react::CallInvokerHolder;
	facebook::jni::alias_ref<Holder::javaobject> holder(
		reinterpret_cast<Holder::javaobject>(callInvokerHolder)
	);
	auto* nativeHolder = facebook::jni::cthis(holder);
	if (nativeHolder == nullptr) return 0;
	auto transport = RuntimeTransport::create(nativeHolder->getCallInvoker());
	auto& runtime = *reinterpret_cast<facebook::jsi::Runtime*>(runtimePointer);
	if (!transport->install(runtime)) return 0;
	return reinterpret_cast<jlong>(new std::shared_ptr<RuntimeTransport>(std::move(transport)));
}

extern "C" JNIEXPORT void JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeInvalidateTransport(
	JNIEnv*,
	jclass,
	jlong token
) {
	auto* box = transportBox(token);
	if (box == nullptr) return;
	if (*box) (*box)->invalidate();
	delete box;
}

extern "C" JNIEXPORT jlong JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeConfigureEngine(
	JNIEnv* env,
	jclass,
	jstring storagePath,
	jstring defaultRelays,
	jstring indexerRelays,
	jboolean meshEnabled
) {
	return reinterpret_cast<jlong>(EngineHost::shared().configure(
		fromJString(env, storagePath),
		fromJString(env, defaultRelays),
		fromJString(env, indexerRelays),
		meshEnabled == JNI_TRUE
	));
}

extern "C" JNIEXPORT void JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeDeinitEngine(
	JNIEnv*,
	jclass
) {
	EngineHost::shared().deinit();
}

extern "C" JNIEXPORT void JNICALL
Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeShutdownProcessEngine(
	JNIEnv*,
	jclass
) {
	EngineHost::shared().shutdownProcess();
}
