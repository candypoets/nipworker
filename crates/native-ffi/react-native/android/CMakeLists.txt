cmake_minimum_required(VERSION 3.13)

project(nipworker_react_native)

add_library(nipworker_react_native SHARED
	src/main/cpp/NipworkerByteRuntime.cpp
)

find_package(ReactAndroid REQUIRED CONFIG)

target_include_directories(nipworker_react_native PRIVATE
	${REACT_ANDROID_DIR}/../
)

target_link_libraries(nipworker_react_native
	ReactAndroid::jsi
	android
)

target_link_options(nipworker_react_native PRIVATE "-Wl,--allow-shlib-undefined")
