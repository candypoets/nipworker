#!/bin/bash

set -e

echo "Building Nostr Main WASM module..."

# Build main thread WASM module
echo ""
echo "Building main thread WASM module..."
wasm-pack build \
    --target web \
    --out-dir pkg \
    --out-name nostr_main \
    --release \
    --features wee_alloc

# Generate size report
echo ""
echo "Build complete! Files generated:"
echo ""
ls -la pkg/

echo ""
echo "Main WASM module built successfully!"
echo ""
echo "Generated files:"
echo "  - pkg/nostr_main.js (JavaScript bindings)"
echo "  - pkg/nostr_main_bg.wasm (WebAssembly module)"
echo "  - pkg/nostr_main.d.ts (TypeScript definitions)"

# Final size report
MAIN_SIZE=$(wc -c < pkg/nostr_main_bg.wasm)

echo ""
echo "Final size:"
echo "Main module: $MAIN_SIZE bytes ($(echo "scale=2; $MAIN_SIZE/1024" | bc -l)KB)"

echo ""
echo "✅ Nostr Main build complete!"
