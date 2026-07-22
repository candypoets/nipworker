cmake_minimum_required(VERSION 3.13)

project(nipworker_react_native)

include("${REACT_NATIVE_DIR}/ReactAndroid/cmake-utils/folly-flags.cmake")
add_compile_options(${folly_FLAGS})

add_library(nipworker_react_native SHARED
	src/main/cpp/NipworkerByteRuntime.cpp
)

find_package(ReactAndroid REQUIRED CONFIG)
find_package(nipworker-native-ffi-android REQUIRED CONFIG)

target_include_directories(nipworker_react_native PRIVATE
	${REACT_NATIVE_DIR}/ReactCommon
	${REACT_NATIVE_DIR}/ReactCommon/jsi
)

target_link_libraries(nipworker_react_native
	nipworker-native-ffi-android::nipworker_native_ffi
	ReactAndroid::reactnative
	ReactAndroid::jsi
	android
)

target_link_options(nipworker_react_native PRIVATE "-Wl,--allow-shlib-undefined")
