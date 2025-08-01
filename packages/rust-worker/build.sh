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

# Build worker WASM module
echo ""
echo "Building worker WASM module..."
wasm-pack build \
    --target web \

# Optimize worker WASM
# optimize_wasm "pkg/nostr_worker_bg.wasm"

# Create worker.js file
echo ""
echo "Creating worker.js..."
cat > pkg/worker.js << 'EOF'
import init, { init_nostr_client } from "./rust_worker.js";

const initPromise = async () => {
  try {
    console.log("WASM worker module initialized successfully");
    await init();
    return await init_nostr_client();
  } catch (error) {
    console.log("oops");
    console.error("Failed to initialize WASM worker module:", error);
    throw error;
  }
};

const wasmReady = initPromise();

self.onmessage = async (event) => {
  (await wasmReady).handle_message(event.data);
};
EOF

# Update package.json to include worker.js in files array using sed
echo "Updating package.json..."
PACKAGE_JSON="pkg/package.json"

# Check if worker.js is already in the files array
if ! grep -q '"worker.js"' "$PACKAGE_JSON"; then
    echo "Adding worker.js to package.json files array..."

    # Use sed to add comma to the last item before the closing bracket, then add worker.js
    sed -i.bak '
        /^[[:space:]]*"files":[[:space:]]*\[/,/^[[:space:]]*\]/ {
            /^[[:space:]]*\]/ {
                i\
    "worker.js"
            }
            /^[[:space:]]*"[^"]*"[[:space:]]*$/ {
                /^[[:space:]]*\]/ !s/$/,/
            }
        }
    ' "$PACKAGE_JSON"

    # Clean up backup file
    rm "${PACKAGE_JSON}.bak"

    echo "Added worker.js to package.json files array"
else
    echo "worker.js already present in package.json files array"
fi

# Clean up worker.js backup
if [ -f "worker.js.backup" ]; then
    rm worker.js.backup
fi

# Generate size report
echo ""
echo "Build complete! Files generated:"
echo ""
ls -la pkg/

echo ""
echo "Worker WASM module built successfully!"
echo ""
echo "Generated files:"
echo "  - pkg/rust_worker.js (JavaScript bindings)"
echo "  - pkg/rust_worker_bg.wasm (WebAssembly module)"
echo "  - pkg/rust_worker.d.ts (TypeScript definitions)"
echo "  - pkg/worker.js (Web Worker wrapper)"

# Final size report
WORKER_SIZE=$(wc -c < pkg/rust_worker_bg.wasm)

echo ""
echo "Final size:"
echo "Worker module: $WORKER_SIZE bytes ($(echo "scale=2; $WORKER_SIZE/1024" | bc -l)KB)"

echo ""
echo "✅ Nostr Worker build complete!"
