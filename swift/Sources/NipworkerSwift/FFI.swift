import Darwin
import Foundation
import ObjectiveC.runtime

// MARK: - C FFI Imports from libnipworker_native_ffi

@_silgen_name("nipworker_init")
func nipworker_init(
    _ callback: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, Int) -> Void,
    _ userdata: UnsafeMutableRawPointer?
) -> UnsafeMutableRawPointer?

@_silgen_name("nipworker_handle_message")
func nipworker_handle_message(
    _ handle: UnsafeMutableRawPointer?,
    _ ptr: UnsafePointer<UInt8>?,
    _ len: Int
)

@_silgen_name("nipworker_subscribe_message")
func nipworker_subscribe_message(
    _ handle: UnsafeMutableRawPointer?,
    _ ptr: UnsafePointer<UInt8>?,
    _ len: Int
) -> Bool

@_silgen_name("nipworker_publish_message")
func nipworker_publish_message(
    _ handle: UnsafeMutableRawPointer?,
    _ ptr: UnsafePointer<UInt8>?,
    _ len: Int
) -> Bool

@_silgen_name("nipworker_set_private_key")
func nipworker_set_private_key(
    _ handle: UnsafeMutableRawPointer?,
    _ ptr: UnsafePointer<Int8>?
)

@_silgen_name("nipworker_deinit")
func nipworker_deinit(_ handle: UnsafeMutableRawPointer?)

@_silgen_name("nipworker_free_bytes")
func nipworker_free_bytes(_ ptr: UnsafeMutablePointer<UInt8>?, _ len: Int)

@_silgen_name("nipworker_register_subscription")
func nipworker_register_subscription(
    _ handle: UnsafeMutableRawPointer?,
    _ subId: UnsafePointer<Int8>?,
    _ bufferSize: Int
) -> Bool

@_silgen_name("nipworker_register_publish_buffer")
func nipworker_register_publish_buffer(
    _ handle: UnsafeMutableRawPointer?,
    _ publishId: UnsafePointer<Int8>?,
    _ bufferSize: Int
) -> Bool

@_silgen_name("nipworker_retain_subscription")
func nipworker_retain_subscription(
    _ handle: UnsafeMutableRawPointer?,
    _ subId: UnsafePointer<Int8>?
) -> Bool

@_silgen_name("nipworker_release_subscription")
func nipworker_release_subscription(
    _ handle: UnsafeMutableRawPointer?,
    _ subId: UnsafePointer<Int8>?
)

@_silgen_name("nipworker_subscription_buffer_ptr")
func nipworker_subscription_buffer_ptr(
    _ handle: UnsafeMutableRawPointer?,
    _ subId: UnsafePointer<Int8>?
) -> UnsafeMutablePointer<UInt8>?

@_silgen_name("nipworker_subscription_buffer_len")
func nipworker_subscription_buffer_len(
    _ handle: UnsafeMutableRawPointer?,
    _ subId: UnsafePointer<Int8>?
) -> Int

@_silgen_name("nipworker_cleanup_subscriptions")
func nipworker_cleanup_subscriptions(_ handle: UnsafeMutableRawPointer?)

func nipworker_react_native_shared_handle_if_available() -> UnsafeMutableRawPointer? {
    let runtimeClassName = "NipworkerRuntime"
    let sharedHandleSelector = NSSelectorFromString("sharedHandle")
    if let runtimeClass = NSClassFromString(runtimeClassName), runtimeClass.responds(to: sharedHandleSelector) {
        typealias SharedHandleMessageSend = @convention(c) (AnyClass, Selector) -> UnsafeMutableRawPointer?
        if let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "objc_msgSend") {
            let send = unsafeBitCast(symbol, to: SharedHandleMessageSend.self)
            if let handle = send(runtimeClass, sharedHandleSelector) {
                return handle
            }
        }
    }

    typealias SharedHandleFunction = @convention(c) () -> UnsafeMutableRawPointer?

    guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "nipworker_react_native_shared_handle") else {
        return nil
    }

    return unsafeBitCast(symbol, to: SharedHandleFunction.self)()
}
