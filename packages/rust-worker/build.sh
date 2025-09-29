#!/bin/bash

set -e

echo "Building Nostr Worker WASM module..."

# Install wasm-pack if not available (includes automatic wasm-opt support if Binaryen is installed)
if ! command -v wasm-pack &> /dev/null; then
    echo "wasm-pack not found. Installing..."
    curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
fi

# Note: wasm-opt (from Binaryen) is used automatically by wasm-pack for release builds if installed.
# Install via: brew install binaryen (macOS), apt install binaryen (Ubuntu), etc.

# Always use full release build for maximum performance
echo "Building with full release optimizations..."
wasm-pack build --release --target web

# Verify WASM file was generated
if [ -f "pkg/rust_worker_bg.wasm" ]; then
    echo "✅ WASM build completed successfully!"
else
    echo "❌ WASM file not found - build may have failed"
    exit 1
fi

# Create worker.js file
echo ""
echo "Creating worker.js..."
cat > pkg/worker.js << 'EOF'
import init, { init_nostr_client } from "./rust_worker.js";

let client;

// Pre-initialize for faster first message handling
const initPromise = async (config) => {
  try {
    console.log("Initializing WASM worker module...");
    if (config.wasmBuffer) {
      // Use provided buffer (passed from main thread) - create synthetic Response for init
      const wasmResponse = new Response(config.wasmBuffer, {
        headers: { "Content-Type": "application/wasm" }
      });
      console.log("init using wasmResponse");
      // Modern init call to suppress deprecated warning: pass { module: response }
      await init({ module_or_path: wasmResponse });
    } else {
      // Fallback to default fetch (for standalone testing)
      await init();
    }
    const clientInstance = await init_nostr_client(config.bufferKey, config.maxBufferSize, config.inRing, config.outRing);
    console.log("WASM worker module initialized successfully");
    return clientInstance;
  } catch (error) {
    console.error("Failed to initialize WASM worker module:", error);
    throw error;
  }
};

// Handle messages
self.onmessage = async (event) => {
  try {
    if (event.data.type === "init") {
      const config = event.data.payload;
      client = initPromise(config); // client is now the promise
      console.log("Worker init started - messages will await client promise");
    } else {
      // All non-init messages: await the client promise, then process
      client.then((c) => {
        c.handle_message(event.data);
      }).catch((error) => {
        console.error("Worker message processing error (client failed):", error);
        self.postMessage({
          type: "error",
          error: error.toString()
        });
      });
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
echo "  - Full Rust optimization (opt-level=3, full LTO, 1 codegen unit)"
echo "  - wasm-opt (via wasm-pack, if installed): -O, strip debug/producers, enable bulk-memory/sign-ext/etc."
echo "  - Debug symbols stripped (Rust-side)"
echo "  - Producer info stripped (wasm-opt)"

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
echo "   Built with full performance optimizations"
