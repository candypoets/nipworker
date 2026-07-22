#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AAR_PATH="${1:-$SCRIPT_DIR/build/outputs/aar/nipworker-native-ffi-android-release.aar}"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

ABIS=(
	"arm64-v8a"
	"armeabi-v7a"
	"x86"
	"x86_64"
)

SYMBOLS=(
	"JNI_OnLoad"
	"nipworker_init"
	"nipworker_handle_message"
	"nipworker_set_private_key"
	"nipworker_deinit"
	"nipworker_free_bytes"
	"Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerInit"
	"Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerHandleMessage"
	"Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerSetPrivateKey"
	"Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerDeinit"
	"Java_com_candypoets_nipworker_reactnative_NipworkerReactNativeModule_nipworkerFreeBytes"
)

if [[ ! -f "$AAR_PATH" ]]; then
	echo "AAR not found: $AAR_PATH" >&2
	exit 1
fi

unzip -q "$AAR_PATH" -d "$TMP_DIR"

if ! unzip -l "$AAR_PATH" | grep -q "classes.jar"; then
	echo "classes.jar missing from AAR" >&2
	exit 1
fi

if [[ ! -f "$TMP_DIR/prefab/prefab.json" ]]; then
	echo "Prefab package metadata missing from AAR" >&2
	exit 1
fi
if [[ ! -f "$TMP_DIR/prefab/modules/nipworker_native_ffi/include/nipworker.h" ]]; then
	echo "Prefab public header missing from AAR" >&2
	exit 1
fi
if ! grep -q '"name": "nipworker-native-ffi-android"' "$TMP_DIR/prefab/prefab.json"; then
	echo "Unexpected Prefab package name" >&2
	exit 1
fi

nm_bin="${ANDROID_NM:-}"
if [[ -z "$nm_bin" ]]; then
	host_tag=""
	case "$(uname -s)" in
		Darwin) host_tag="darwin-x86_64" ;;
		Linux) host_tag="linux-x86_64" ;;
	esac
	for ndk_root in "${ANDROID_NDK_HOME:-}" "${ANDROID_NDK_ROOT:-}"; do
		if [[ -n "$host_tag" && -n "$ndk_root" && -x "$ndk_root/toolchains/llvm/prebuilt/$host_tag/bin/llvm-nm" ]]; then
			nm_bin="$ndk_root/toolchains/llvm/prebuilt/$host_tag/bin/llvm-nm"
			break
		fi
	done
fi
if [[ -z "$nm_bin" ]]; then
	if command -v llvm-nm >/dev/null 2>&1; then
		nm_bin="$(command -v llvm-nm)"
	else
		nm_bin="$(command -v nm)"
	fi
fi

for abi in "${ABIS[@]}"; do
	lib="$TMP_DIR/jni/$abi/libnipworker_native_ffi.so"
	prefab_lib="$TMP_DIR/prefab/modules/nipworker_native_ffi/libs/android.$abi/libnipworker_native_ffi.so"
	if [[ ! -f "$lib" ]]; then
		echo "missing $lib in AAR" >&2
		exit 1
	fi
	if [[ ! -f "$prefab_lib" ]]; then
		echo "missing Prefab library for $abi" >&2
		exit 1
	fi
	if ! cmp -s "$lib" "$prefab_lib"; then
		echo "JNI and Prefab libraries differ for $abi" >&2
		exit 1
	fi
	symbol_output="$("$nm_bin" -D "$lib" 2>/dev/null)"

	for symbol in "${SYMBOLS[@]}"; do
		if ! grep -q "[[:space:]]$symbol$" <<< "$symbol_output"; then
			echo "missing symbol $symbol in $abi/libnipworker_native_ffi.so" >&2
			exit 1
		fi
	done
done

echo "AAR validation passed: $AAR_PATH"
