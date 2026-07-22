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
#       rustup target add aarch64-apple-darwin x86_64-apple-darwin

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET_DIR="${CRATE_DIR}/target"
IOS_DIR="${SCRIPT_DIR}"
IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-14.0}"
MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

echo "=== NIPWorker iOS Build ==="
echo "Crate dir: ${CRATE_DIR}"
echo "iOS deployment target: ${IPHONEOS_DEPLOYMENT_TARGET}"
echo "macOS deployment target: ${MACOSX_DEPLOYMENT_TARGET}"
echo ""

# ── Check Rust targets ──────────────────────────────────────────────
REQUIRED_TARGETS=(
	"aarch64-apple-ios"
	"aarch64-apple-ios-sim"
	"x86_64-apple-ios"
	"aarch64-apple-darwin"
	"x86_64-apple-darwin"
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
	local rustflags=""

	case "${target}" in
		aarch64-apple-ios)
			rustflags="-C link-arg=-miphoneos-version-min=${IPHONEOS_DEPLOYMENT_TARGET}"
			;;
		aarch64-apple-ios-sim|x86_64-apple-ios)
			rustflags="-C link-arg=-mios-simulator-version-min=${IPHONEOS_DEPLOYMENT_TARGET}"
			;;
		aarch64-apple-darwin|x86_64-apple-darwin)
			rustflags="-C link-arg=-mmacosx-version-min=${MACOSX_DEPLOYMENT_TARGET}"
			;;
	esac

	echo ""
	echo "Building for ${target}..."
	cd "${CRATE_DIR}"
	IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET}" \
		MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET}" \
		RUSTFLAGS="${RUSTFLAGS:-} ${rustflags}" \
		cargo build --release --target "${target}"
}

build_target "aarch64-apple-ios"
build_target "aarch64-apple-ios-sim"
build_target "x86_64-apple-ios"
build_target "aarch64-apple-darwin"
build_target "x86_64-apple-darwin"

# ── Create simulator fat binary ─────────────────────────────────────
echo ""
echo "Creating simulator fat binary (arm64 + x86_64)..."
mkdir -p "${IOS_DIR}/Frameworks/ios-arm64"
mkdir -p "${IOS_DIR}/Frameworks/ios-arm64_x86_64-simulator"
mkdir -p "${IOS_DIR}/Frameworks/macos-arm64_x86_64"

lipo -create \
	"${TARGET_DIR}/aarch64-apple-ios-sim/release/libnipworker_native_ffi.a" \
	"${TARGET_DIR}/x86_64-apple-ios/release/libnipworker_native_ffi.a" \
	-output "${IOS_DIR}/Frameworks/ios-arm64_x86_64-simulator/libnipworker_native_ffi.a"

echo "  -> ${IOS_DIR}/Frameworks/ios-arm64_x86_64-simulator/libnipworker_native_ffi.a"

echo ""
echo "Creating macOS fat binary (arm64 + x86_64)..."
lipo -create \
	"${TARGET_DIR}/aarch64-apple-darwin/release/libnipworker_native_ffi.a" \
	"${TARGET_DIR}/x86_64-apple-darwin/release/libnipworker_native_ffi.a" \
	-output "${IOS_DIR}/Frameworks/macos-arm64_x86_64/libnipworker_native_ffi.a"

echo "  -> ${IOS_DIR}/Frameworks/macos-arm64_x86_64/libnipworker_native_ffi.a"

# ── Copy device binary ──────────────────────────────────────────────
cp "${TARGET_DIR}/aarch64-apple-ios/release/libnipworker_native_ffi.a" \
	"${IOS_DIR}/Frameworks/ios-arm64/libnipworker_native_ffi.a"
echo "  -> ${IOS_DIR}/Frameworks/ios-arm64/libnipworker_native_ffi.a"

# ── Create XCFramework ──────────────────────────────────────────────
echo ""
echo "Creating XCFramework..."
xcodebuild -create-xcframework \
	-library "${IOS_DIR}/Frameworks/ios-arm64/libnipworker_native_ffi.a" \
	-headers "${CRATE_DIR}/include" \
	-library "${IOS_DIR}/Frameworks/ios-arm64_x86_64-simulator/libnipworker_native_ffi.a" \
	-headers "${CRATE_DIR}/include" \
	-library "${IOS_DIR}/Frameworks/macos-arm64_x86_64/libnipworker_native_ffi.a" \
	-headers "${CRATE_DIR}/include" \
	-output "${IOS_DIR}/NipworkerNativeFFI.xcframework"

echo "  -> ${IOS_DIR}/NipworkerNativeFFI.xcframework"

# ── Summary ─────────────────────────────────────────────────────────
echo ""
echo "=== Build Complete ==="
echo ""
echo "Artifacts:"
echo "  XCFramework:      ${IOS_DIR}/NipworkerNativeFFI.xcframework"
echo ""
echo "Integration: add NipworkerNativeFFI.xcframework to your link phase"
echo "  and use 'Do Not Embed' (it contains static libraries)"
