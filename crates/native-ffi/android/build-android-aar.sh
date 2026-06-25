#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
JNICALL_DIR="$SCRIPT_DIR/src/main/jniLibs"

ABIS=(
	"arm64-v8a"
	"armeabi-v7a"
	"x86"
	"x86_64"
)

TARGETS=(
	"aarch64-linux-android"
	"armv7-linux-androideabi"
	"i686-linux-android"
	"x86_64-linux-android"
)

find_strip() {
	local host_tag
	case "$(uname -s)" in
		Darwin) host_tag="darwin-x86_64" ;;
		Linux) host_tag="linux-x86_64" ;;
		*) host_tag="" ;;
	esac

	if [[ -n "$host_tag" && -n "${ANDROID_NDK_HOME:-}" && -x "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin/llvm-strip" ]]; then
		echo "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin/llvm-strip"
		return
	fi
	if [[ -n "$host_tag" && -n "${ANDROID_NDK_ROOT:-}" && -x "$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/$host_tag/bin/llvm-strip" ]]; then
		echo "$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/$host_tag/bin/llvm-strip"
		return
	fi
	if command -v llvm-strip >/dev/null 2>&1; then
		command -v llvm-strip
		return
	fi
	echo ""
}

cd "$CRATE_DIR"
cargo ndk \
	-t armeabi-v7a \
	-t arm64-v8a \
	-t x86 \
	-t x86_64 \
	-o "$JNICALL_DIR" \
	build --release

strip_bin="$(find_strip)"
for index in "${!ABIS[@]}"; do
	abi="${ABIS[$index]}"
	target="${TARGETS[$index]}"
	source_lib="$CRATE_DIR/target/$target/release/libnipworker_native_ffi.so"
	output_lib="$JNICALL_DIR/$abi/libnipworker_native_ffi.so"

	mkdir -p "$(dirname "$output_lib")"
	cp "$source_lib" "$output_lib"

	if [[ -n "$strip_bin" ]]; then
		"$strip_bin" --strip-unneeded "$output_lib"
	else
		echo "warning: llvm-strip not found; leaving $output_lib unstripped" >&2
	fi
done

cd "$SCRIPT_DIR"
if [[ -x "./gradlew" ]]; then
	./gradlew assembleRelease
else
	gradle assembleRelease
fi
