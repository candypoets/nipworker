#!/bin/bash

set -e

echo "Building Nostr Worker WASM module..."

# Install required tools if not available
if ! command -v wasm-pack &> /dev/null; then
    echo "wasm-pack not found. Installing..."
    curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
fi

if ! command -v wasm-opt &> /dev/null; then
    echo "wasm-opt not found. Please install binaryen for additional WASM optimization:"
    echo "  - macOS: brew install binaryen"
    echo "  - Ubuntu/Debian: apt install binaryen"
    echo "  - Windows: Download from https://github.com/WebAssembly/binaryen/releases"
    echo "Continuing without additional wasm-opt optimizations..."
fi

# Function to optimize WASM file
optimize_wasm() {
    local file_path="$1"
    local backup_path="${file_path}.backup"

    if command -v wasm-opt &> /dev/null; then
        echo "Optimizing $file_path..."

        # Get original size
        ORIGINAL_SIZE=$(wc -c < "$file_path")
        echo "Original size: $ORIGINAL_SIZE bytes"

        # Create backup
        cp "$file_path" "$backup_path"

        # Apply Safari-compatible optimizations
        wasm-opt -Oz --enable-bulk-memory --enable-sign-ext --enable-mutable-globals \
            --strip-debug --strip-producers \
            "$backup_path" -o "$file_path"

        # Get optimized size
        OPTIMIZED_SIZE=$(wc -c < "$file_path")
        REDUCTION=$((ORIGINAL_SIZE - OPTIMIZED_SIZE))
        PERCENT_REDUCTION=$((REDUCTION * 100 / ORIGINAL_SIZE))

        echo "Optimized size: $OPTIMIZED_SIZE bytes"
        echo "Reduction: $REDUCTION bytes ($PERCENT_REDUCTION%)"

        # Remove backup
        rm "$backup_path"
    else
        echo "Skipping optimization for $file_path (wasm-opt not available)"
    fi
}

# Clean previous builds
echo "Cleaning previous builds..."
rm -rf pkg/

# Build worker WASM module
echo ""
echo "Building worker WASM module..."
wasm-pack build \
    --target web \
    --out-dir pkg \
    --out-name nutscash_nostr_worker \
    --release \
    --features console_error_panic_hook

# Optimize worker WASM
optimize_wasm "pkg/nutscash_nostr_worker_bg.wasm"

# Generate size report
echo ""
echo "Build complete! Files generated:"
echo ""
ls -la pkg/

echo ""
echo "Worker WASM module built successfully!"
echo ""
echo "Generated files:"
echo "  - pkg/nutscash_nostr_worker.js (JavaScript bindings)"
echo "  - pkg/nutscash_nostr_worker_bg.wasm (WebAssembly module)"
echo "  - pkg/nutscash_nostr_worker.d.ts (TypeScript definitions)"

# Final size report
WORKER_SIZE=$(wc -c < pkg/nutscash_nostr_worker_bg.wasm)

echo ""
echo "Final size:"
echo "Worker module: $WORKER_SIZE bytes ($(echo "scale=2; $WORKER_SIZE/1024" | bc -l)KB)"

echo ""
echo "✅ Nostr Worker build complete!"
