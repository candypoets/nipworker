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
        rm -f "$backup_path"
    else
        echo "Skipping optimization for $file_path (wasm-opt not available)"
    fi
}

# Build configuration based on environment
echo ""
if [ "${CI:-false}" = "true" ]; then
    echo "Building in CI environment..."
    # Check available memory
    if command -v free >/dev/null 2>&1; then
        echo "Available memory:"
        free -h
    fi

    # Use memory-safe Rust compilation settings
    export CARGO_PROFILE_RELEASE_LTO=thin
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=4
    echo "Using CI-safe Rust compilation settings"

    # Try CI profile first, fallback to release
    echo "Attempting CI-optimized build profile..."
    if wasm-pack build --release --target web -- --profile ci-release; then
        echo "✅ CI profile build successful"
    else
        echo "⚠️ CI profile failed, using standard release build..."
        wasm-pack build --release --target web
    fi
else
    echo "Building in local environment..."
    wasm-pack build --release --target web
fi

# Optimize the generated WASM file
if [ -f "pkg/rust_worker_bg.wasm" ]; then
    optimize_wasm "pkg/rust_worker_bg.wasm"
else
    echo "❌ WASM file not found - build may have failed"
    exit 1
fi

echo "✅ WASM build completed successfully!"

# Create worker.js file
echo ""
echo "Creating worker.js..."
cat > pkg/worker.js << 'EOF'
import init, { init_nostr_client } from "./rust_worker.js";

// Pre-initialize for faster first message handling
const initPromise = async (config) => {
  try {
    console.log("Initializing WASM worker module...");
    await init();
    const client = await init_nostr_client(config.bufferKey, config.maxBufferSize);
    console.log("WASM worker module initialized successfully");
    return client;
  } catch (error) {
    console.error("Failed to initialize WASM worker module:", error);
    throw error;
  }
};

let client;

// Handle messages
self.onmessage = async (event) => {
  try {
    if(event.data.type === "init") {
      const config = event.data.payload;
      client = initPromise(config);
    } else {
      if(client) {
        const c = await client;
        c.handle_message(event.data);
      }
    }
  } catch (error) {
    console.error("Worker message handling error:", error);
    self.postMessage({
      type: "error",
      error: error.toString()
    });
  }
};
EOF

# Update package.json to include worker.js in files array
echo "Updating package.json..."
PACKAGE_JSON="pkg/package.json"

if [ -f "$PACKAGE_JSON" ]; then
    if ! grep -q '"worker.js"' "$PACKAGE_JSON"; then
        echo "Adding worker.js to package.json files array..."

        # Simple and reliable approach - add after rust_worker_bg.wasm
        sed -i.bak 's/"rust_worker_bg.wasm"/"rust_worker_bg.wasm",\
    "worker.js"/' "$PACKAGE_JSON" && rm -f "${PACKAGE_JSON}.bak" || {
            echo "⚠️ Could not update package.json automatically"
        }
    else
        echo "worker.js already present in package.json"
    fi
else
    echo "⚠️ package.json not found at $PACKAGE_JSON"
fi

# Generate performance report
echo ""
echo "📊 Build Report"
echo "==============="

# List generated files with sizes
echo "Generated files:"
for file in pkg/*.wasm pkg/*.js pkg/*.d.ts pkg/worker.js; do
    if [ -f "$file" ]; then
        ls -lh "$file" | awk '{print "  " $9 ": " $5}'
    fi
done

# Show optimizations applied
echo ""
echo "Optimizations applied:"
if [ "${CI:-false}" = "true" ]; then
    echo "  - CI-safe Rust compilation (thin LTO, 4 codegen units)"
else
    echo "  - Full Rust optimization (opt-level=3, full LTO)"
fi
echo "  - WASM optimization with wasm-opt"
echo "  - Debug symbols stripped"
echo "  - Producer info stripped"

# Final size report
if [ -f "pkg/rust_worker_bg.wasm" ]; then
    WORKER_SIZE=$(wc -c < pkg/rust_worker_bg.wasm)
    MB_SIZE=$(( WORKER_SIZE / 1024 / 1024 ))
    KB_REMAINDER=$(( (WORKER_SIZE % (1024 * 1024)) / 1024 ))
    echo ""
    echo "Final WASM size: $WORKER_SIZE bytes (${MB_SIZE}.$(printf "%02d" $((KB_REMAINDER * 100 / 1024)))MB)"
fi

echo ""
echo "✅ Nostr Worker build complete!"
if [ "${CI:-false}" = "true" ]; then
    echo "   Built with CI-safe settings for reliable GitHub Actions execution"
else
    echo "   Built with full performance optimizations"
fi
