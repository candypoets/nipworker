/**
 * HarmonyOS NAPI C++ bridge for NIPWorker.
 *
 * This file translates between HarmonyOS NAPI (Node-API based) and the
 * Rust C ABI exposed by libnipworker_native_ffi.
 *
 * Build (HarmonyOS toolchain):
 *   DevEco Studio compiles this automatically when placed in the
 *   cpp/ directory of a HarmonyOS module and referenced in build-profile.json5.
 *
 * Build (local smoke-test with Node.js headers):
 *   g++ -std=c++17 -I$(node -p "require('path').dirname(process.execPath)+'/../include/node'") \
 *       -fPIC -shared -o nipworker_napi.so nipworker_napi.cpp \
 *       -L../../target/release -lnipworker_native_ffi
 */

#include <cstring>
#include <cstdint>
#include <cstdlib>

// ---------------------------------------------------------------------------
// NAPI headers (HarmonyOS NAPI is based on Node-API)
// ---------------------------------------------------------------------------
#include <node_api.h>

// ---------------------------------------------------------------------------
// Rust C FFI declarations
// ---------------------------------------------------------------------------
extern "C" {
void* nipworker_init(
    void (*callback)(void* userdata, const uint8_t* ptr, size_t len),
    void* userdata);
void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
void nipworker_set_private_key(void* handle, const char* ptr);
void nipworker_deinit(void* handle);
void nipworker_free_bytes(uint8_t* ptr, size_t len);
}

// ---------------------------------------------------------------------------
// Thread-safe callback state
// ---------------------------------------------------------------------------
struct TsfnState {
    napi_threadsafe_function tsfn;
};

static void call_js_cb(napi_env env, napi_value js_callback, void* context, void* data) {
    (void)context;

    if (env == nullptr || js_callback == nullptr || data == nullptr) {
        return;
    }

    // Data layout from Rust: [sub_id_len:4][sub_id][payload_len:4][payload]
    uint8_t* packet = static_cast<uint8_t*>(data);
    uint32_t sub_id_len = 0;
    std::memcpy(&sub_id_len, packet, sizeof(uint32_t));

    size_t offset = sizeof(uint32_t);
    if (offset + sub_id_len + sizeof(uint32_t) > 0xFFFFFF) {
        // Sanity check failed
        nipworker_free_bytes(packet, 0);
        return;
    }

    uint32_t payload_len = 0;
    std::memcpy(&payload_len, packet + offset + sub_id_len, sizeof(uint32_t));

    size_t total_len = sizeof(uint32_t) + sub_id_len + sizeof(uint32_t) + payload_len;

    // Create ArrayBuffer for payload only
    napi_value arraybuffer;
    uint8_t* js_buf = nullptr;
    napi_status s = napi_create_arraybuffer(env, payload_len, reinterpret_cast<void**>(&js_buf), &arraybuffer);
    if (s != napi_ok || js_buf == nullptr) {
        nipworker_free_bytes(packet, static_cast<size_t>(total_len));
        return;
    }

    std::memcpy(js_buf, packet + sizeof(uint32_t) + sub_id_len + sizeof(uint32_t), payload_len);
    nipworker_free_bytes(packet, static_cast<size_t>(total_len));

    // Invoke JS callback(ArrayBuffer)
    napi_value undefined;
    napi_get_undefined(env, &undefined);
    napi_call_function(env, undefined, js_callback, 1, &arraybuffer, nullptr);
}

static void native_callback(void* userdata, const uint8_t* ptr, size_t len) {
    TsfnState* state = static_cast<TsfnState*>(userdata);
    if (state == nullptr || state->tsfn == nullptr) {
        nipworker_free_bytes(const_cast<uint8_t*>(ptr), len);
        return;
    }

    napi_status s = napi_call_threadsafe_function(
        state->tsfn,
        const_cast<uint8_t*>(ptr), // ownership transferred to JS thread
        napi_tsfn_nonblocking);

    if (s != napi_ok) {
        // Queue full or shutting down — free the buffer ourselves
        nipworker_free_bytes(const_cast<uint8_t*>(ptr), len);
    }
}

// ---------------------------------------------------------------------------
// NAPI methods exposed to ArkTS
// ---------------------------------------------------------------------------

static napi_value napi_nipworker_init(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, nullptr, nullptr);

    if (argc < 1) {
        napi_throw_type_error(env, nullptr, "init expects a callback");
        return nullptr;
    }

    // Validate callback
    napi_valuetype valuetype;
    napi_typeof(env, args[0], &valuetype);
    if (valuetype != napi_function) {
        napi_throw_type_error(env, nullptr, "init expects a function callback");
        return nullptr;
    }

    // Create thread-safe function for Rust → ArkTS callbacks
    napi_value resource_name;
    napi_create_string_utf8(env, "nipworker_callback", NAPI_AUTO_LENGTH, &resource_name);

    TsfnState* state = new TsfnState{};

    napi_status s = napi_create_threadsafe_function(
        env,
        args[0],
        nullptr,
        resource_name,
        0,               // max_queue_size (0 = unlimited)
        1,               // initial_thread_count
        state,           // thread_finalize_data
        [](napi_env /*env*/, void* finalize_data, void* /*finalize_hint*/) {
            TsfnState* st = static_cast<TsfnState*>(finalize_data);
            delete st;
        },
        state,
        call_js_cb,
        &state->tsfn);

    if (s != napi_ok) {
        delete state;
        napi_throw_error(env, nullptr, "Failed to create threadsafe function");
        return nullptr;
    }

    void* handle = nipworker_init(native_callback, state);

    napi_value result;
    napi_create_bigint_int64(env, reinterpret_cast<int64_t>(handle), &result);
    return result;
}

static napi_value napi_nipworker_handle_message(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, nullptr, nullptr);

    if (argc < 2) {
        napi_throw_type_error(env, nullptr, "handleMessage expects (handle, bytes)");
        return nullptr;
    }

    // Extract handle
    int64_t handle_val = 0;
    napi_get_value_bigint_int64(env, args[0], &handle_val, nullptr);

    // Extract ArrayBuffer
    bool is_arraybuffer = false;
    napi_is_arraybuffer(env, args[1], &is_arraybuffer);
    if (!is_arraybuffer) {
        napi_throw_type_error(env, nullptr, "handleMessage expects an ArrayBuffer");
        return nullptr;
    }

    uint8_t* data = nullptr;
    size_t byte_length = 0;
    napi_get_arraybuffer_info(env, args[1], reinterpret_cast<void**>(&data), &byte_length);

    nipworker_handle_message(reinterpret_cast<void*>(handle_val), data, byte_length);

    napi_value undefined;
    napi_get_undefined(env, &undefined);
    return undefined;
}

static napi_value napi_nipworker_set_private_key(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, nullptr, nullptr);

    if (argc < 2) {
        napi_throw_type_error(env, nullptr, "setPrivateKey expects (handle, secret)");
        return nullptr;
    }

    int64_t handle_val = 0;
    napi_get_value_bigint_int64(env, args[0], &handle_val, nullptr);

    size_t str_len = 0;
    napi_get_value_string_utf8(env, args[1], nullptr, 0, &str_len);
    char* secret = static_cast<char*>(std::malloc(str_len + 1));
    napi_get_value_string_utf8(env, args[1], secret, str_len + 1, &str_len);

    nipworker_set_private_key(reinterpret_cast<void*>(handle_val), secret);
    std::free(secret);

    napi_value undefined;
    napi_get_undefined(env, &undefined);
    return undefined;
}

static napi_value napi_nipworker_deinit(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, nullptr, nullptr);

    int64_t handle_val = 0;
    napi_get_value_bigint_int64(env, args[0], &handle_val, nullptr);

    nipworker_deinit(reinterpret_cast<void*>(handle_val));

    napi_value undefined;
    napi_get_undefined(env, &undefined);
    return undefined;
}

static napi_value napi_nipworker_free_bytes(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, nullptr, nullptr);

    int64_t ptr_val = 0;
    napi_get_value_bigint_int64(env, args[0], &ptr_val, nullptr);

    int64_t len_val = 0;
    napi_get_value_bigint_int64(env, args[1], &len_val, nullptr);

    if (ptr_val != 0 && len_val > 0) {
        nipworker_free_bytes(reinterpret_cast<uint8_t*>(ptr_val), static_cast<size_t>(len_val));
    }

    napi_value undefined;
    napi_get_undefined(env, &undefined);
    return undefined;
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

static napi_value init_module(napi_env env, napi_value exports) {
    napi_property_descriptor descs[] = {
        { "nipworkerInit", nullptr, napi_nipworker_init, nullptr, nullptr, nullptr, napi_default, nullptr },
        { "nipworkerHandleMessage", nullptr, napi_nipworker_handle_message, nullptr, nullptr, nullptr, napi_default, nullptr },
        { "nipworkerSetPrivateKey", nullptr, napi_nipworker_set_private_key, nullptr, nullptr, nullptr, napi_default, nullptr },
        { "nipworkerDeinit", nullptr, napi_nipworker_deinit, nullptr, nullptr, nullptr, napi_default, nullptr },
        { "nipworkerFreeBytes", nullptr, napi_nipworker_free_bytes, nullptr, nullptr, nullptr, napi_default, nullptr },
    };

    napi_define_properties(env, exports, sizeof(descs) / sizeof(descs[0]), descs);
    return exports;
}

NAPI_MODULE(NODE_GYP_MODULE_NAME, init_module)
