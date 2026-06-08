/*
 * JNI C bridge for NIPWorker Android (LynxJS).
 *
 * CRITICAL FIXES applied:
 * 1. JNI_OnLoad caches JavaVM* g_vm (previously never set, causing
 *    native_callback to silently drop all Rust events).
 * 2. RegisterNatives maps the Kotlin external methods to the impl_ functions
 *    (avoids relying on dynamic symbol resolution for the impl_ prefix).
 * 3. g_cls and g_mid are cached once in JNI_OnLoad instead of on first
 *    nipworkerInit call.
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
extern void* nipworker_init_with_storage_path(
    void (*callback)(void* userdata, const uint8_t* ptr, size_t len),
    void* userdata,
    const char* storage_path
);
extern void* nipworker_init_with_config(
    void (*callback)(void* userdata, const uint8_t* ptr, size_t len),
    void* userdata,
    const char* storage_path,
    const char* default_relays,
    const char* indexer_relays
);
extern void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
extern void nipworker_set_private_key(void* handle, const char* ptr);
extern void nipworker_deinit(void* handle);
extern void nipworker_free_bytes(uint8_t* ptr, size_t len);

/* Cached JNI globals */
static JavaVM* g_vm = NULL;
static jclass g_cls = NULL;
static jmethodID g_mid = NULL;

/* Prevent the linker from garbage-collecting JNI entry points. */
#define JNI_USED __attribute__((used, visibility("default")))

/* Forward declarations */
static void native_callback(void* userdata, const uint8_t* ptr, size_t len);

/* Fallback init: JNI_OnLoad may be stripped by the linker, so we
 * lazily initialise the cached JNI globals on the first nipworkerInit(). */
static void ensure_jni_cache(JNIEnv* env, jclass cls) {
    if (g_vm != NULL && g_cls != NULL && g_mid != NULL) return;

    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionClear(env);
    }

    jint ret = (*env)->GetJavaVM(env, &g_vm);
    if (ret != 0 || g_vm == NULL) return;

    g_cls = (jclass)(*env)->NewGlobalRef(env, cls);
    g_mid = (*env)->GetStaticMethodID(env, g_cls, "onNativeData", "(J[B)V");
}

JNIEXPORT jlong JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerInit(
    JNIEnv* env, jclass cls, jlong userdata);
JNIEXPORT jlong JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithStoragePath(
    JNIEnv* env, jclass cls, jlong userdata, jstring storage_path);
JNIEXPORT jlong JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithConfig(
    JNIEnv* env,
    jclass cls,
    jlong userdata,
    jstring storage_path,
    jstring default_relays,
    jstring indexer_relays);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerHandleMessage(
    JNIEnv* env, jclass cls, jlong handle, jbyteArray bytes);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerSetPrivateKey(
    JNIEnv* env, jclass cls, jlong handle, jstring secret);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerDeinit(
    JNIEnv* env, jclass cls, jlong handle);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerFreeBytes(
    JNIEnv* env, jclass cls, jlong ptr, jlong len);

/* ---------------------------------------------------------------------------
 * JNI_OnLoad – called when the shared library is loaded.
 * Caches the JavaVM* and registers all native methods explicitly.
 * --------------------------------------------------------------------------- */
JNI_USED
JNIEXPORT jint JNICALL impl_JNI_OnLoad(JavaVM* vm, void* reserved) {
    g_vm = vm;

    JNIEnv* env = NULL;
    if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) != JNI_OK) {
        return JNI_ERR;
    }

    jclass cls = (*env)->FindClass(
        env,
        "com/candypoets/nipworker/lynx/NipworkerLynxModule"
    );
    if (cls == NULL) {
        if ((*env)->ExceptionCheck(env)) {
            (*env)->ExceptionClear(env);
        }
        return JNI_VERSION_1_6;
    }

    /* Cache global class ref and onNativeData method ID for callbacks */
    g_cls = (jclass)(*env)->NewGlobalRef(env, cls);
    g_mid = (*env)->GetStaticMethodID(env, g_cls, "onNativeData", "(J[B)V");
    (*env)->DeleteLocalRef(env, cls);

    if (g_mid == NULL) {
        return JNI_ERR;
    }

    static JNINativeMethod methods[] = {
        {
            "nipworkerInit",
            "(J)J",
            (void*)&impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerInit
        },
        {
            "nipworkerHandleMessage",
            "(J[B)V",
            (void*)&impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerHandleMessage
        },
        {
            "nipworkerSetPrivateKey",
            "(JLjava/lang/String;)V",
            (void*)&impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerSetPrivateKey
        },
        {
            "nipworkerDeinit",
            "(J)V",
            (void*)&impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerDeinit
        },
        {
            "nipworkerFreeBytes",
            "(JJ)V",
            (void*)&impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerFreeBytes
        },
    };

    jint ret = (*env)->RegisterNatives(
        env, g_cls, methods,
        sizeof(methods) / sizeof(methods[0])
    );
    if (ret < 0) {
        return JNI_ERR;
    }

    return JNI_VERSION_1_6;
}

/* ---------------------------------------------------------------------------
 * JNI_OnUnload – clean up global refs.
 * --------------------------------------------------------------------------- */
JNI_USED
JNIEXPORT void JNICALL impl_JNI_OnUnload(JavaVM* vm, void* reserved) {
    JNIEnv* env = NULL;
    if ((*vm)->GetEnv(vm, (void**)&env, JNI_VERSION_1_6) == JNI_OK) {
        if (g_cls != NULL) {
            (*env)->DeleteGlobalRef(env, g_cls);
            g_cls = NULL;
        }
    }
    g_vm = NULL;
    g_mid = NULL;
}

/* ---------------------------------------------------------------------------
 * Native method implementations (impl_ prefix so we can register them
 * explicitly via RegisterNatives instead of relying on JNI name mangling).
 * --------------------------------------------------------------------------- */

JNI_USED
JNIEXPORT jlong JNICALL
impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerInit(
    JNIEnv* env,
    jclass cls,
    jlong userdata
) {
    ensure_jni_cache(env, cls);
    void* handle = nipworker_init(native_callback, (void*)(uintptr_t)userdata);
    return (jlong)handle;
}

JNI_USED
JNIEXPORT jlong JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithStoragePath(
    JNIEnv* env,
    jclass cls,
    jlong userdata,
    jstring storage_path
) {
    return impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithConfig(
        env,
        cls,
        userdata,
        storage_path,
        NULL,
        NULL
    );
}

JNI_USED
JNIEXPORT jlong JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithConfig(
    JNIEnv* env,
    jclass cls,
    jlong userdata,
    jstring storage_path,
    jstring default_relays,
    jstring indexer_relays
) {
    ensure_jni_cache(env, cls);
    const char* cpath = NULL;
    const char* cdefault_relays = NULL;
    const char* cindexer_relays = NULL;
    if (storage_path != NULL) {
        cpath = (*env)->GetStringUTFChars(env, storage_path, NULL);
    }
    if (default_relays != NULL) {
        cdefault_relays = (*env)->GetStringUTFChars(env, default_relays, NULL);
    }
    if (indexer_relays != NULL) {
        cindexer_relays = (*env)->GetStringUTFChars(env, indexer_relays, NULL);
    }
    void* handle = nipworker_init_with_config(
        native_callback,
        (void*)(uintptr_t)userdata,
        cpath,
        cdefault_relays,
        cindexer_relays
    );
    if (storage_path != NULL && cpath != NULL) {
        (*env)->ReleaseStringUTFChars(env, storage_path, cpath);
    }
    if (default_relays != NULL && cdefault_relays != NULL) {
        (*env)->ReleaseStringUTFChars(env, default_relays, cdefault_relays);
    }
    if (indexer_relays != NULL && cindexer_relays != NULL) {
        (*env)->ReleaseStringUTFChars(env, indexer_relays, cindexer_relays);
    }
    return (jlong)handle;
}

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

/* ---------------------------------------------------------------------------
 * Rust callback forwarder.
 * Copies data into a JVM byte[] and invokes Kotlin onNativeData().
 * --------------------------------------------------------------------------- */
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
