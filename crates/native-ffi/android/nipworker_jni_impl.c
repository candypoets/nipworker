/* Cold-path JNI commands and mesh bridge for Android.
 * React Native event delivery and engine creation live in the shared C++ JSI
 * transport; this file never calls Java/Kotlin from a Rust worker thread. */

#include <jni.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Rust C API declarations */
extern void nipworker_set_log_level(const char* level);
extern void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
extern void nipworker_set_private_key(void* handle, const char* ptr);
extern void nipworker_clear_signer(void* handle);
extern void nipworker_remove_signer(void* handle);
extern void nipworker_wake(void* handle);
extern void nipworker_free_bytes(uint8_t* ptr, size_t len);
extern bool nipworker_mesh_peer_connected(void* handle, const char* peer, size_t mtu);
extern void nipworker_mesh_peer_disconnected(void* handle, const char* peer);
extern uint8_t* nipworker_mesh_pop_outbound(void* handle, const char* peer, size_t* out_len);
extern bool nipworker_mesh_receive_fragment(void* handle, const char* peer, const uint8_t* fragment, size_t fragment_len);
extern bool nipworker_mesh_set_profile_json(void* handle, const char* profile_json);
extern bool nipworker_mesh_clear_profile(void* handle);

/* Prevent the linker from garbage-collecting JNI entry points. */
#define JNI_USED __attribute__((used, visibility("default")))

JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerSetLogLevel(
    JNIEnv* env, jclass cls, jstring log_level);
JNIEXPORT jboolean JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshPeerConnected(
    JNIEnv* env, jclass cls, jlong handle, jstring peer, jint mtu);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshPeerDisconnected(
    JNIEnv* env, jclass cls, jlong handle, jstring peer);
JNIEXPORT jboolean JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshReceiveFragment(
    JNIEnv* env, jclass cls, jlong handle, jstring peer, jbyteArray fragment);
JNIEXPORT jbyteArray JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshPopOutbound(
    JNIEnv* env, jclass cls, jlong handle, jstring peer);
JNIEXPORT jboolean JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshSetProfile(
    JNIEnv* env, jclass cls, jlong handle, jstring profile_json);
JNIEXPORT jboolean JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshClearProfile(
    JNIEnv* env, jclass cls, jlong handle);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerHandleMessage(
    JNIEnv* env, jclass cls, jlong handle, jbyteArray bytes);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerSetPrivateKey(
    JNIEnv* env, jclass cls, jlong handle, jstring secret);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerClearSigner(
    JNIEnv* env, jclass cls, jlong handle);
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerRemoveSigner(
    JNIEnv* env, jclass cls, jlong handle);

JNI_USED
JNIEXPORT jint JNICALL impl_JNI_OnLoad(JavaVM* vm, void* reserved) {
	(void)vm;
	(void)reserved;
    return JNI_VERSION_1_6;
}

JNI_USED
JNIEXPORT jboolean JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshPeerConnected(
    JNIEnv* env, jclass cls, jlong handle, jstring peer, jint mtu
) {
    if (handle == 0 || peer == NULL || mtu <= 17) return JNI_FALSE;
    const char* cpeer = (*env)->GetStringUTFChars(env, peer, NULL);
    if (cpeer == NULL) return JNI_FALSE;
    bool ok = nipworker_mesh_peer_connected((void*)handle, cpeer, (size_t)mtu);
    (*env)->ReleaseStringUTFChars(env, peer, cpeer);
    return ok ? JNI_TRUE : JNI_FALSE;
}

JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshPeerDisconnected(
    JNIEnv* env, jclass cls, jlong handle, jstring peer
) {
    if (handle == 0 || peer == NULL) return;
    const char* cpeer = (*env)->GetStringUTFChars(env, peer, NULL);
    if (cpeer == NULL) return;
    nipworker_mesh_peer_disconnected((void*)handle, cpeer);
    (*env)->ReleaseStringUTFChars(env, peer, cpeer);
}

JNI_USED
JNIEXPORT jboolean JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshReceiveFragment(
    JNIEnv* env, jclass cls, jlong handle, jstring peer, jbyteArray fragment
) {
    if (handle == 0 || peer == NULL || fragment == NULL) return JNI_FALSE;
    const char* cpeer = (*env)->GetStringUTFChars(env, peer, NULL);
    if (cpeer == NULL) return JNI_FALSE;
    jsize len = (*env)->GetArrayLength(env, fragment);
    jbyte* bytes = (*env)->GetByteArrayElements(env, fragment, NULL);
    bool ok = bytes != NULL && nipworker_mesh_receive_fragment(
        (void*)handle, cpeer, (const uint8_t*)bytes, (size_t)len
    );
    if (bytes != NULL) (*env)->ReleaseByteArrayElements(env, fragment, bytes, JNI_ABORT);
    (*env)->ReleaseStringUTFChars(env, peer, cpeer);
    return ok ? JNI_TRUE : JNI_FALSE;
}

JNI_USED
JNIEXPORT jbyteArray JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshPopOutbound(
    JNIEnv* env, jclass cls, jlong handle, jstring peer
) {
    if (handle == 0 || peer == NULL) return NULL;
    const char* cpeer = (*env)->GetStringUTFChars(env, peer, NULL);
    if (cpeer == NULL) return NULL;
    size_t len = 0;
    uint8_t* bytes = nipworker_mesh_pop_outbound((void*)handle, cpeer, &len);
    (*env)->ReleaseStringUTFChars(env, peer, cpeer);
    if (bytes == NULL || len == 0 || len > INT32_MAX) {
        if (bytes != NULL) nipworker_free_bytes(bytes, len);
        return NULL;
    }
    jbyteArray result = (*env)->NewByteArray(env, (jsize)len);
    if (result != NULL) {
        (*env)->SetByteArrayRegion(env, result, 0, (jsize)len, (const jbyte*)bytes);
    }
    nipworker_free_bytes(bytes, len);
    return result;
}

JNI_USED
JNIEXPORT jboolean JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshSetProfile(
    JNIEnv* env, jclass cls, jlong handle, jstring profile_json
) {
    if (handle == 0 || profile_json == NULL) return JNI_FALSE;
    const char* json = (*env)->GetStringUTFChars(env, profile_json, NULL);
    if (json == NULL) return JNI_FALSE;
    bool ok = nipworker_mesh_set_profile_json((void*)handle, json);
    (*env)->ReleaseStringUTFChars(env, profile_json, json);
    return ok ? JNI_TRUE : JNI_FALSE;
}

JNI_USED
JNIEXPORT jboolean JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nativeMeshClearProfile(
    JNIEnv* env, jclass cls, jlong handle
) {
    return handle != 0 && nipworker_mesh_clear_profile((void*)handle) ? JNI_TRUE : JNI_FALSE;
}

JNI_USED
JNIEXPORT void JNICALL impl_JNI_OnUnload(JavaVM* vm, void* reserved) {
	(void)vm;
	(void)reserved;
}

/* ---------------------------------------------------------------------------
 * Native method implementations (impl_ prefix so we can register them
 * explicitly via RegisterNatives instead of relying on JNI name mangling).
 * --------------------------------------------------------------------------- */

JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerSetLogLevel(
    JNIEnv* env,
    jclass cls,
    jstring log_level
) {
    const char* clevel = NULL;
    if (log_level != NULL) {
        clevel = (*env)->GetStringUTFChars(env, log_level, NULL);
    }
    nipworker_set_log_level(clevel);
    if (log_level != NULL && clevel != NULL) {
        (*env)->ReleaseStringUTFChars(env, log_level, clevel);
    }
}

JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerHandleMessage(
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
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerSetPrivateKey(
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
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerClearSigner(
    JNIEnv* env,
    jclass cls,
    jlong handle
) {
    (void)env;
    (void)cls;
    if (handle != 0) {
        nipworker_clear_signer((void*)handle);
    }
}

JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerRemoveSigner(
    JNIEnv* env,
    jclass cls,
    jlong handle
) {
    (void)env;
    (void)cls;
    if (handle != 0) {
        nipworker_remove_signer((void*)handle);
    }
}

JNI_USED
JNIEXPORT void JNICALL
impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerWake(
    JNIEnv* env,
    jclass cls,
    jlong handle
) {
    if (handle == 0) return;
    nipworker_wake((void*)handle);
}
