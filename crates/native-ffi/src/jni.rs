//! JNI bridge for Android (LynxJS).
//!
//! These `#[no_mangle]` functions are the entry points called from
//! Kotlin via the JVM's JNI. Because they are defined in Rust with
//! `#[no_mangle]`, rustc's generated version script exports them
//! automatically when building a `cdylib`.
//!
//! The actual implementation lives in `android/nipworker_jni_impl.c`;
//! these thin Rust wrappers just forward to the `_impl_*` C functions.

use std::ffi::c_void;

extern "C" {
    fn impl_JNI_OnLoad(vm: *mut c_void, reserved: *mut c_void) -> i32;

    fn impl_JNI_OnUnload(vm: *mut c_void, reserved: *mut c_void);

    fn impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerInit(
        env: *mut c_void,
        cls: *mut c_void,
        userdata: i64,
    ) -> i64;

    fn impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithStoragePath(
        env: *mut c_void,
        cls: *mut c_void,
        userdata: i64,
        storage_path: *mut c_void,
    ) -> i64;

    fn impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithConfig(
        env: *mut c_void,
        cls: *mut c_void,
        userdata: i64,
        storage_path: *mut c_void,
        default_relays: *mut c_void,
        indexer_relays: *mut c_void,
    ) -> i64;

    fn impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerHandleMessage(
        env: *mut c_void,
        cls: *mut c_void,
        handle: i64,
        bytes: *mut c_void,
    );

    fn impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerSetPrivateKey(
        env: *mut c_void,
        cls: *mut c_void,
        handle: i64,
        secret: *mut c_void,
    );

    fn impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerDeinit(
        env: *mut c_void,
        cls: *mut c_void,
        handle: i64,
    );

    fn impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerWake(
        env: *mut c_void,
        cls: *mut c_void,
        handle: i64,
    );

    fn impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerFreeBytes(
        env: *mut c_void,
        cls: *mut c_void,
        ptr: i64,
        len: i64,
    );
}

#[no_mangle]
pub extern "C" fn JNI_OnLoad(vm: *mut c_void, reserved: *mut c_void) -> i32 {
    unsafe { impl_JNI_OnLoad(vm, reserved) }
}

#[no_mangle]
pub extern "C" fn JNI_OnUnload(vm: *mut c_void, reserved: *mut c_void) {
    unsafe { impl_JNI_OnUnload(vm, reserved) }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerInit(
    env: *mut c_void,
    cls: *mut c_void,
    userdata: i64,
) -> i64 {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerInit(
            env, cls, userdata,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerHandleMessage(
    env: *mut c_void,
    cls: *mut c_void,
    handle: i64,
    bytes: *mut c_void,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerHandleMessage(
            env, cls, handle, bytes,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerSetPrivateKey(
    env: *mut c_void,
    cls: *mut c_void,
    handle: i64,
    secret: *mut c_void,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerSetPrivateKey(
            env, cls, handle, secret,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerDeinit(
    env: *mut c_void,
    cls: *mut c_void,
    handle: i64,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerDeinit(
            env, cls, handle,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerFreeBytes(
    env: *mut c_void,
    cls: *mut c_void,
    ptr: i64,
    len: i64,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerFreeBytes(
            env, cls, ptr, len,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInit(
    env: *mut c_void,
    cls: *mut c_void,
    userdata: i64,
) -> i64 {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerInit(
            env, cls, userdata,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithStoragePath(
    env: *mut c_void,
    cls: *mut c_void,
    userdata: i64,
    storage_path: *mut c_void,
) -> i64 {
    unsafe {
        impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithStoragePath(
            env,
            cls,
            userdata,
            storage_path,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithConfig(
    env: *mut c_void,
    cls: *mut c_void,
    userdata: i64,
    storage_path: *mut c_void,
    default_relays: *mut c_void,
    indexer_relays: *mut c_void,
) -> i64 {
    unsafe {
        impl_Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInitWithConfig(
            env,
            cls,
            userdata,
            storage_path,
            default_relays,
            indexer_relays,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerHandleMessage(
    env: *mut c_void,
    cls: *mut c_void,
    handle: i64,
    bytes: *mut c_void,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerHandleMessage(
            env, cls, handle, bytes,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerSetPrivateKey(
    env: *mut c_void,
    cls: *mut c_void,
    handle: i64,
    secret: *mut c_void,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerSetPrivateKey(
            env, cls, handle, secret,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerDeinit(
    env: *mut c_void,
    cls: *mut c_void,
    handle: i64,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerDeinit(
            env, cls, handle,
        )
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerWake(
    env: *mut c_void,
    cls: *mut c_void,
    handle: i64,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerWake(env, cls, handle)
    }
}

#[no_mangle]
pub extern "C" fn Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerFreeBytes(
    env: *mut c_void,
    cls: *mut c_void,
    ptr: i64,
    len: i64,
) {
    unsafe {
        impl_Java_com_candypoets_nipworker_lynx_NipworkerLynxModule_nipworkerFreeBytes(
            env, cls, ptr, len,
        )
    }
}
