import { defineConfig } from "vite";
import { resolve } from "path";
import dts from "vite-plugin-dts";
import wasm from "vite-plugin-wasm";

export default defineConfig({
  plugins: [
    wasm(),
    dts({
      include: ["src/**/*"],
      exclude: ["src/**/*.test.*", "src/**/*.spec.*"],
      outDir: "dist",
      insertTypesEntry: true,
    }),
  ],
  build: {
    lib: {
      entry: resolve(__dirname, "src/index.ts"),
      name: "NostrWorkerLib",
      formats: ["es", "umd"],
      fileName: (format) => `index.${format === "es" ? "js" : "umd.cjs"}`,
    },
    rollupOptions: {
      external: ["@msgpack/msgpack", "nostr-tools", "msgpackr"],
      output: {
        globals: {
          "@msgpack/msgpack": "MessagePack",
          "nostr-tools": "NostrTools",
          msgpackr: "Msgpackr",
        },
        // Ensure worker and WASM files are properly handled
        assetFileNames: (assetInfo) => {
          if (assetInfo.name?.endsWith(".wasm")) {
            return "wasm/[name][extname]";
          }
          if (assetInfo.name?.includes("worker")) {
            return "workers/[name][extname]";
          }
          return "assets/[name][extname]";
        },
      },
    },
    target: "es2020",
    minify: "esbuild",
    sourcemap: true,
  },
  worker: {
    format: "es",
    plugins: () => [wasm()],
  },
  assetsInclude: ["**/*.wasm"],
  optimizeDeps: {
    exclude: ["@msgpack/msgpack", "nostr-tools", "msgpackr"],
  },
  define: {
    "process.env.NODE_ENV": '"production"',
  },
});
