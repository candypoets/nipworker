/*
 * JNI C bridge for NIPWorker Android (LynxJS).
 *
 * This file translates between JNI types and the Rust C ABI exposed by
 * libnipworker_native_ffi. It is compiled as a static library
 * (libnipworker_jni.a) and linked into the final Android binary.
 *
 * The Kotlin side (LynxNipworkerModule.kt) calls the `external` methods
 * declared below, which forward to the Rust functions.
 *
 * Callback path: Rust C callback -> native_callback() ->
 *   JNIEnv->CallStaticVoidMethod -> LynxNipworkerModule.onNativeData()
 */

#include <jni.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Rust C API declarations */
extern void* nipworker_init(
    void (*callback)(void* userdata, const uint8_t* ptr, size_t len),
    void* userdata
);
extern void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
extern void nipworker_set_private_key(void* handle, const char* ptr);
extern void nipworker_deinit(void* handle);
extern void nipworker_free_bytes(uint8_t* ptr, size_t len);

/* Cached JNI globals */
static JavaVM* g_vm = NULL;
static jclass g_cls = NULL;
static jmethodID g_mid = NULL;

/* Forward declaration */
static void native_callback(void* userdata, const uint8_t* ptr, size_t len);

/* Prevent the linker from garbage-collecting JNI entry points. */
#define JNI_USED __attribute__((used, visibility("default")))

JNI_USED

/*
 * Class:     com_candypoets_nipworker_lynx_NipworkerLynxModule
 * Method:    nipworkerInit
 * Signature: (J)J
 */
JNI_USED
JNIEXPORT jlong JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerInit(
    JNIEnv* env,
    jclass cls,
    jlong userdata
) {
    /* Cache class reference and method ID on first call */
    if (g_cls == NULL) {
        g_cls = (*env)->NewGlobalRef(env, cls);
        g_mid = (*env)->GetStaticMethodID(
            env, cls,
            "onNativeData",
            "(J[B)V"
        );
        if (g_mid == NULL) {
            return 0;
        }
    }

    void* handle = nipworker_init(native_callback, (void*)(uintptr_t)userdata);
    return (jlong)handle;
}

/*
 * Class:     com_candypoets_nipworker_lynx_NipworkerLynxModule
 * Method:    nipworkerHandleMessage
 * Signature: (J[B)V
 */
JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerHandleMessage(
    JNIEnv* env,
    jclass cls,
    jlong handle,
    jbyteArray bytes
) {
    if (handle == 0 || bytes == NULL) return;

    jsize len = (*env)->GetArrayLength(env, bytes);
    jbyte* ptr = (*env)->GetByteArrayElements(env, bytes, NULL);
    if (ptr == NULL) return;

    nipworker_handle_message((void*)handle, (const uint8_t*)ptr, (size_t)len);

    (*env)->ReleaseByteArrayElements(env, bytes, ptr, JNI_ABORT);
}

/*
 * Class:     com_candypoets_nipworker_lynx_NipworkerLynxModule
 * Method:    nipworkerSetPrivateKey
 * Signature: (JLjava/lang/String;)V
 */
JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerSetPrivateKey(
    JNIEnv* env,
    jclass cls,
    jlong handle,
    jstring secret
) {
    if (handle == 0 || secret == NULL) return;

    const char* cstr = (*env)->GetStringUTFChars(env, secret, NULL);
    if (cstr == NULL) return;

    nipworker_set_private_key((void*)handle, cstr);

    (*env)->ReleaseStringUTFChars(env, secret, cstr);
}

/*
 * Class:     com_candypoets_nipworker_lynx_NipworkerLynxModule
 * Method:    nipworkerDeinit
 * Signature: (J)V
 */
JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerDeinit(
    JNIEnv* env,
    jclass cls,
    jlong handle
) {
    if (handle == 0) return;
    nipworker_deinit((void*)handle);
}

/*
 * Class:     com_candypoets_nipworker_lynx_NipworkerLynxModule
 * Method:    nipworkerFreeBytes
 * Signature: (JJ)V
 */
JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerFreeBytes(
    JNIEnv* env,
    jclass cls,
    jlong ptr,
    jlong len
) {
    if (ptr == 0 || len == 0) return;
    nipworker_free_bytes((uint8_t*)(uintptr_t)ptr, (size_t)len);
}

/*
 * Rust callback forwarder.
 * Copies data into a JVM byte[] and invokes Kotlin onNativeData().
 */
static void native_callback(void* userdata, const uint8_t* ptr, size_t len) {
    if (g_vm == NULL || g_cls == NULL || g_mid == NULL || ptr == NULL || len == 0) {
        nipworker_free_bytes((uint8_t*)ptr, len);
        return;
    }

    JNIEnv* env = NULL;
    jint attach = (*g_vm)->GetEnv(g_vm, (void**)&env, JNI_VERSION_1_6);
    if (attach == JNI_EDETACHED) {
        if ((*g_vm)->AttachCurrentThread(g_vm, &env, NULL) != 0) {
            nipworker_free_bytes((uint8_t*)ptr, len);
            return;
        }
    } else if (attach != JNI_OK) {
        nipworker_free_bytes((uint8_t*)ptr, len);
        return;
    }

    jbyteArray arr = (*env)->NewByteArray(env, (jsize)len);
    if (arr != NULL) {
        (*env)->SetByteArrayRegion(env, arr, 0, (jsize)len, (const jbyte*)ptr);
        (*env)->CallStaticVoidMethod(
            env, g_cls, g_mid,
            (jlong)(uintptr_t)userdata,
            arr
        );
        (*env)->DeleteLocalRef(env, arr);
    }

    nipworker_free_bytes((uint8_t*)ptr, len);

    if (attach == JNI_EDETACHED) {
        (*g_vm)->DetachCurrentThread(g_vm);
    }
}
