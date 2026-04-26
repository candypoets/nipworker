#!/bin/bash
set -euo pipefail

# build-ios.sh
# Builds libnipworker_native_ffi.a and NipworkerNativeFFI.xcframework
# for iOS device and simulator targets.
#
# Usage:
#   cd crates/native-ffi/ios
#   ./build-ios.sh
#
# Prerequisites:
#   - macOS with Xcode 15+
#   - Rust toolchain with iOS targets:
#       rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET_DIR="${CRATE_DIR}/target"
IOS_DIR="${SCRIPT_DIR}"

echo "=== NIPWorker iOS Build ==="
echo "Crate dir: ${CRATE_DIR}"
echo ""

# ── Check Rust targets ──────────────────────────────────────────────
REQUIRED_TARGETS=(
	"aarch64-apple-ios"
	"aarch64-apple-ios-sim"
	"x86_64-apple-ios"
)

MISSING_TARGETS=()
for target in "${REQUIRED_TARGETS[@]}"; do
	if ! rustup target list --installed | grep -q "${target}"; then
		MISSING_TARGETS+=("${target}")
	fi
done

if [ ${#MISSING_TARGETS[@]} -ne 0 ]; then
	echo "Error: Missing Rust targets: ${MISSING_TARGETS[*]}"
	echo "Install them with:"
	echo "  rustup target add ${MISSING_TARGETS[*]}"
	exit 1
fi

# ── Clean previous builds ───────────────────────────────────────────
echo "Cleaning previous iOS artifacts..."
rm -rf "${IOS_DIR}/Frameworks"
rm -f "${IOS_DIR}/libnipworker_native_ffi.a"
rm -rf "${IOS_DIR}/NipworkerNativeFFI.xcframework"

# ── Build for each target ───────────────────────────────────────────
build_target() {
	local target=$1
	echo ""
	echo "Building for ${target}..."
	cd "${CRATE_DIR}"
	cargo build --release --target "${target}"
}

build_target "aarch64-apple-ios"
build_target "aarch64-apple-ios-sim"
build_target "x86_64-apple-ios"

# ── Create simulator fat binary ─────────────────────────────────────
echo ""
echo "Creating simulator fat binary (arm64 + x86_64)..."
mkdir -p "${IOS_DIR}/Frameworks"

lipo -create \
	"${TARGET_DIR}/aarch64-apple-ios-sim/release/libnipworker_native_ffi.a" \
	"${TARGET_DIR}/x86_64-apple-ios/release/libnipworker_native_ffi.a" \
	-output "${IOS_DIR}/Frameworks/libnipworker_native_ffi_sim.a"

echo "  -> ${IOS_DIR}/Frameworks/libnipworker_native_ffi_sim.a"

# ── Copy device binary ──────────────────────────────────────────────
cp "${TARGET_DIR}/aarch64-apple-ios/release/libnipworker_native_ffi.a" \
	"${IOS_DIR}/Frameworks/libnipworker_native_ffi_device.a"
echo "  -> ${IOS_DIR}/Frameworks/libnipworker_native_ffi_device.a"

# ── Create universal .a (device + sim) ──────────────────────────────
echo ""
echo "Creating universal static library..."
lipo -create \
	"${IOS_DIR}/Frameworks/libnipworker_native_ffi_device.a" \
	"${IOS_DIR}/Frameworks/libnipworker_native_ffi_sim.a" \
	-output "${IOS_DIR}/libnipworker_native_ffi.a"

echo "  -> ${IOS_DIR}/libnipworker_native_ffi.a"

# ── Create XCFramework ──────────────────────────────────────────────
echo ""
echo "Creating XCFramework..."
xcodebuild -create-xcframework \
	-library "${IOS_DIR}/Frameworks/libnipworker_native_ffi_device.a" \
	-library "${IOS_DIR}/Frameworks/libnipworker_native_ffi_sim.a" \
	-output "${IOS_DIR}/NipworkerNativeFFI.xcframework"

echo "  -> ${IOS_DIR}/NipworkerNativeFFI.xcframework"

# ── Summary ─────────────────────────────────────────────────────────
echo ""
echo "=== Build Complete ==="
echo ""
echo "Artifacts:"
echo "  Universal .a:     ${IOS_DIR}/libnipworker_native_ffi.a"
echo "  XCFramework:      ${IOS_DIR}/NipworkerNativeFFI.xcframework"
echo ""
echo "Integration options:"
echo "  1. CocoaPods (podspec): links libnipworker_native_ffi.a automatically"
echo "  2. Manual Xcode: drag NipworkerNativeFFI.xcframework into your project"
echo "     and set 'Embed & Sign'"
