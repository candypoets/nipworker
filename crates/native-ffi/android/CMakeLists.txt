cmake_minimum_required(VERSION 3.22.1)

project(nipworker_native_ffi_android)

# cargo-ndk produces the real library before Gradle runs. AGP's Prefab
# publisher only packages outputs built by CMake, so build a tiny placeholder
# target and replace its output with the Rust library after linking.
add_library(nipworker_native_ffi SHARED src/main/cpp/prefab_stub.c)
add_custom_command(TARGET nipworker_native_ffi POST_BUILD
	COMMAND ${CMAKE_COMMAND} -E copy_if_different
		"${CMAKE_CURRENT_SOURCE_DIR}/src/main/jniLibs/${ANDROID_ABI}/libnipworker_native_ffi.so"
		"$<TARGET_FILE:nipworker_native_ffi>"
)
