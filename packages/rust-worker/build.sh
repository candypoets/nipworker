#!/bin/bash

set -e

echo "Building Nostr Worker WASM module (Performance Optimized)..."

# Install required tools if not available
if ! command -v wasm-pack &> /dev/null; then
    echo "wasm-pack not found. Installing..."
    curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
fi

if ! command -v wasm-opt &> /dev/null; then
    echo "wasm-opt not found. Please install binaryen for WASM optimization:"
    echo "  - macOS: brew install binaryen"
    echo "  - Ubuntu/Debian: apt install binaryen"
    echo "  - Windows: Download from https://github.com/WebAssembly/binaryen/releases"
    echo "Continuing without wasm-opt optimizations..."
fi

# Function to optimize WASM file for performance
optimize_wasm() {
    local file_path="$1"
    local backup_path="${file_path}.backup"

    if command -v wasm-opt &> /dev/null; then
        echo "Optimizing $file_path for performance..."

        # Get original size
        ORIGINAL_SIZE=$(wc -c < "$file_path")
        echo "Original size: $ORIGINAL_SIZE bytes"

        # Create backup
        cp "$file_path" "$backup_path"

        # Apply performance-focused optimizations
        # -O3: Optimize for speed, not size
        # --enable-simd: Enable SIMD instructions for crypto operations
        # --enable-nontrapping-float-to-int: Better float performance
        # --inline-functions-with-loops: Inline hot functions
        # --optimize-for-js: Optimize for JavaScript engine patterns
        wasm-opt -O3 \
            --enable-simd \
            --enable-bulk-memory \
            --enable-sign-ext \
            --enable-mutable-globals \
            --enable-nontrapping-float-to-int \
            --inline-functions-with-loops \
            --optimize-for-js \
            --strip-debug \
            --strip-producers \
            "$backup_path" -o "$file_path"

        # Get optimized size
        OPTIMIZED_SIZE=$(wc -c < "$file_path")
        REDUCTION=$((ORIGINAL_SIZE - OPTIMIZED_SIZE))
        if [ $REDUCTION -gt 0 ]; then
            PERCENT_REDUCTION=$((REDUCTION * 100 / ORIGINAL_SIZE))
            echo "Optimized size: $OPTIMIZED_SIZE bytes"
            echo "Size reduction: $REDUCTION bytes ($PERCENT_REDUCTION%)"
        else
            INCREASE=$((OPTIMIZED_SIZE - ORIGINAL_SIZE))
            PERCENT_INCREASE=$((INCREASE * 100 / ORIGINAL_SIZE))
            echo "Optimized size: $OPTIMIZED_SIZE bytes"
            echo "Size increase: $INCREASE bytes (+$PERCENT_INCREASE%) - Normal for performance optimization"
        fi

        # Remove backup
        rm "$backup_path"
    else
        echo "Skipping optimization for $file_path (wasm-opt not available)"
    fi
}

# Build worker WASM module optimized for performance
echo ""
echo "Building worker WASM module (Performance Mode)..."

# Build with wasm-pack in release mode
# Your Cargo.toml already has opt-level=3, lto=true, codegen-units=1
wasm-pack build --release --target web

# Optimize the generated WASM file
if [ -f "pkg/rust_worker_bg.wasm" ]; then
    optimize_wasm "pkg/rust_worker_bg.wasm"
fi

echo "✅ Successfully built for maximum performance!"

# Create worker.js file
echo ""
echo "Creating worker.js..."
cat > pkg/worker.js << 'EOF'
import init, { init_nostr_client } from "./rust_worker.js";

// Pre-initialize for faster first message handling
const initPromise = (async () => {
  try {
    console.log("Initializing WASM worker module...");
    await init();
    const client = await init_nostr_client();
    console.log("WASM worker module initialized successfully");
    return client;
  } catch (error) {
    console.error("Failed to initialize WASM worker module:", error);
    throw error;
  }
})();

// Handle messages
self.onmessage = async (event) => {
  try {
    const client = await initPromise;
    client.handle_message(event.data);
  } catch (error) {
    console.error("Worker message handling error:", error);
    self.postMessage({
      type: "error",
      error: error.toString()
    });
  }
};

// Notify that worker is ready
initPromise.then(() => {
  self.postMessage({ type: "ready" });
}).catch(error => {
  self.postMessage({
    type: "init_error",
    error: error.toString()
  });
});
EOF

# Update package.json to include worker.js in files array
echo "Updating package.json..."
PACKAGE_JSON="pkg/package.json"

# Check if worker.js is already in the files array
if ! grep -q '"worker.js"' "$PACKAGE_JSON"; then
    echo "Adding worker.js to package.json files array..."

    # Add worker.js to files array
    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS
        sed -i '' '/"files":/,/\]/{
            /\[/!b
            :a
            n
            /\]/!ba
            s/\]/\,\
    "worker.js"\
  ]/
        }' "$PACKAGE_JSON"
    else
        # Linux
        sed -i '/"files":/,/\]/{
            /\[/!b
            :a
            n
            /\]/!ba
            s/\]/\,\n    "worker.js"\n  ]/
        }' "$PACKAGE_JSON"
    fi

    echo "Added worker.js to package.json files array"
else
    echo "worker.js already present in package.json files array"
fi

# Generate performance report
echo ""
echo "Build complete! Performance-optimized build generated."
echo ""

# List generated files with sizes
echo "Generated files:"
ls -lh pkg/*.wasm pkg/*.js pkg/*.d.ts pkg/worker.js 2>/dev/null | while read -r line; do
    echo "  $line"
done

# Show what optimizations are being used from Cargo.toml
echo ""
echo "📊 Build optimizations applied (from Cargo.toml):"
echo "  - opt-level = 3 (maximum performance)"
echo "  - lto = true (link-time optimization)"
echo "  - codegen-units = 1 (better optimization)"
echo "  - panic = abort (smaller binary)"
echo "  - strip = true (no debug symbols)"
echo "  - overflow-checks = false (faster arithmetic)"


# Final size report
if [ -f "pkg/rust_worker_bg.wasm" ]; then
    WORKER_SIZE=$(wc -c < pkg/rust_worker_bg.wasm)
    echo "Final WASM size: $(printf "%'d" $WORKER_SIZE) bytes ($(echo "scale=2; $WORKER_SIZE/1024/1024" | bc -l)MB)"
fi

echo ""
echo "✅ Nostr Worker build complete (Performance Mode)!"
echo ""
echo "This build prioritizes:"
echo "  - Fast execution speed"
echo "  - Optimized crypto operations"
echo "  - Better memory access patterns"
echo "  - Improved JavaScript interop"
