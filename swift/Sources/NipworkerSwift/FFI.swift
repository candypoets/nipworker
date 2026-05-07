import Foundation

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

@_silgen_name("nipworker_set_private_key")
func nipworker_set_private_key(
    _ handle: UnsafeMutableRawPointer?,
    _ ptr: UnsafePointer<Int8>?
)

@_silgen_name("nipworker_deinit")
func nipworker_deinit(_ handle: UnsafeMutableRawPointer?)

@_silgen_name("nipworker_free_bytes")
func nipworker_free_bytes(_ ptr: UnsafeMutablePointer<UInt8>?, _ len: Int)
