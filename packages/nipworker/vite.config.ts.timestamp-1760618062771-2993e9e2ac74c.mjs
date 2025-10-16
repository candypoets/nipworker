// vite.config.ts
import { defineConfig } from "file:///Users/duchenethibaut/candypoets/nipworker/node_modules/vite/dist/node/index.js";
import { resolve } from "path";
import dts from "file:///Users/duchenethibaut/candypoets/nipworker/node_modules/vite-plugin-dts/dist/index.mjs";
import wasm from "file:///Users/duchenethibaut/candypoets/nipworker/node_modules/vite-plugin-wasm/exports/import.mjs";
import topLevelAwait from "file:///Users/duchenethibaut/candypoets/nipworker/node_modules/vite-plugin-top-level-await/exports/import.mjs";
var __vite_injected_original_dirname = "/Users/duchenethibaut/candypoets/nipworker/packages/nipworker";
var vite_config_default = defineConfig({
  plugins: [
    wasm(),
    topLevelAwait(),
    dts({
      include: ["src/**/*"],
      exclude: ["src/**/*.test.*", "src/**/*.spec.*"],
      outDir: "dist",
      insertTypesEntry: true,
      entryRoot: "src",
      rollupTypes: false,
      copyDtsFiles: true,
      pathsToAliases: false
    })
  ],
  resolve: {
    alias: {
      src: resolve(__vite_injected_original_dirname, "src")
    }
  },
  build: {
    lib: {
      entry: resolve(__vite_injected_original_dirname, "src/index.ts"),
      name: "NipWorker",
      formats: ["es"],
      fileName: "index.js"
    },
    rollupOptions: {
      external: (id) => {
        if (id.includes("@candypoets/rust-worker")) {
          return true;
        }
        return ["flatbuffers", "nostr-tools"].includes(id);
      },
      input: {
        index: resolve(__vite_injected_original_dirname, "src/index.ts"),
        // types: resolve(__dirname, "src/types/index.ts"),
        utils: resolve(__vite_injected_original_dirname, "src/utils.ts"),
        hooks: resolve(__vite_injected_original_dirname, "src/hooks.ts"),
        ws: resolve(__vite_injected_original_dirname, "src/ws/index.ts"),
        "ws-rust": resolve(__vite_injected_original_dirname, "src/ws-rust/index.ts")
      },
      output: {
        entryFileNames: (chunkInfo) => {
          const entryNameMap = {
            index: "index.js",
            utils: "utils.js",
            hooks: "hooks.js",
            ws: "ws/index.js",
            "ws-rust": "ws-rust/index.js"
          };
          return entryNameMap[chunkInfo.name] || "[name].js";
        },
        // Handle .d.ts files
        chunkFileNames: (chunkInfo) => {
          return "[name].js";
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
        }
      }
    },
    target: "es2022",
    minify: "esbuild",
    sourcemap: true,
    // Prevent inlining of assets - this is key!
    assetsInlineLimit: 0
    // This prevents base64 inlining
  },
  worker: {
    format: "es",
    rollupOptions: {
      external: ["@candypoets/rust-worker", "@candypoets/rust-worker/worker.js"]
    }
  }
});
export {
  vite_config_default as default
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsidml0ZS5jb25maWcudHMiXSwKICAic291cmNlc0NvbnRlbnQiOiBbImNvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9kaXJuYW1lID0gXCIvVXNlcnMvZHVjaGVuZXRoaWJhdXQvY2FuZHlwb2V0cy9uaXB3b3JrZXIvcGFja2FnZXMvbmlwd29ya2VyXCI7Y29uc3QgX192aXRlX2luamVjdGVkX29yaWdpbmFsX2ZpbGVuYW1lID0gXCIvVXNlcnMvZHVjaGVuZXRoaWJhdXQvY2FuZHlwb2V0cy9uaXB3b3JrZXIvcGFja2FnZXMvbmlwd29ya2VyL3ZpdGUuY29uZmlnLnRzXCI7Y29uc3QgX192aXRlX2luamVjdGVkX29yaWdpbmFsX2ltcG9ydF9tZXRhX3VybCA9IFwiZmlsZTovLy9Vc2Vycy9kdWNoZW5ldGhpYmF1dC9jYW5keXBvZXRzL25pcHdvcmtlci9wYWNrYWdlcy9uaXB3b3JrZXIvdml0ZS5jb25maWcudHNcIjtpbXBvcnQgeyBkZWZpbmVDb25maWcgfSBmcm9tICd2aXRlJztcbmltcG9ydCB7IHJlc29sdmUgfSBmcm9tICdwYXRoJztcbmltcG9ydCBkdHMgZnJvbSAndml0ZS1wbHVnaW4tZHRzJztcbmltcG9ydCB3YXNtIGZyb20gJ3ZpdGUtcGx1Z2luLXdhc20nO1xuaW1wb3J0IHRvcExldmVsQXdhaXQgZnJvbSAndml0ZS1wbHVnaW4tdG9wLWxldmVsLWF3YWl0JztcblxuZXhwb3J0IGRlZmF1bHQgZGVmaW5lQ29uZmlnKHtcblx0cGx1Z2luczogW1xuXHRcdHdhc20oKSxcblx0XHR0b3BMZXZlbEF3YWl0KCksXG5cdFx0ZHRzKHtcblx0XHRcdGluY2x1ZGU6IFsnc3JjLyoqLyonXSxcblx0XHRcdGV4Y2x1ZGU6IFsnc3JjLyoqLyoudGVzdC4qJywgJ3NyYy8qKi8qLnNwZWMuKiddLFxuXHRcdFx0b3V0RGlyOiAnZGlzdCcsXG5cdFx0XHRpbnNlcnRUeXBlc0VudHJ5OiB0cnVlLFxuXHRcdFx0ZW50cnlSb290OiAnc3JjJyxcblx0XHRcdHJvbGx1cFR5cGVzOiBmYWxzZSxcblx0XHRcdGNvcHlEdHNGaWxlczogdHJ1ZSxcblx0XHRcdHBhdGhzVG9BbGlhc2VzOiBmYWxzZVxuXHRcdH0pXG5cdF0sXG5cdHJlc29sdmU6IHtcblx0XHRhbGlhczoge1xuXHRcdFx0c3JjOiByZXNvbHZlKF9fZGlybmFtZSwgJ3NyYycpXG5cdFx0fVxuXHR9LFxuXHRidWlsZDoge1xuXHRcdGxpYjoge1xuXHRcdFx0ZW50cnk6IHJlc29sdmUoX19kaXJuYW1lLCAnc3JjL2luZGV4LnRzJyksXG5cdFx0XHRuYW1lOiAnTmlwV29ya2VyJyxcblx0XHRcdGZvcm1hdHM6IFsnZXMnXSxcblx0XHRcdGZpbGVOYW1lOiAnaW5kZXguanMnXG5cdFx0fSxcblx0XHRyb2xsdXBPcHRpb25zOiB7XG5cdFx0XHRleHRlcm5hbDogKGlkKSA9PiB7XG5cdFx0XHRcdC8vIEhhbmRsZSB3b3JrZXIgaW1wb3J0cyBzcGVjaWZpY2FsbHlcblx0XHRcdFx0aWYgKGlkLmluY2x1ZGVzKCdAY2FuZHlwb2V0cy9ydXN0LXdvcmtlcicpKSB7XG5cdFx0XHRcdFx0cmV0dXJuIHRydWU7XG5cdFx0XHRcdH1cblx0XHRcdFx0cmV0dXJuIFsnZmxhdGJ1ZmZlcnMnLCAnbm9zdHItdG9vbHMnXS5pbmNsdWRlcyhpZCk7XG5cdFx0XHR9LFxuXHRcdFx0aW5wdXQ6IHtcblx0XHRcdFx0aW5kZXg6IHJlc29sdmUoX19kaXJuYW1lLCAnc3JjL2luZGV4LnRzJyksXG5cdFx0XHRcdC8vIHR5cGVzOiByZXNvbHZlKF9fZGlybmFtZSwgXCJzcmMvdHlwZXMvaW5kZXgudHNcIiksXG5cdFx0XHRcdHV0aWxzOiByZXNvbHZlKF9fZGlybmFtZSwgJ3NyYy91dGlscy50cycpLFxuXHRcdFx0XHRob29rczogcmVzb2x2ZShfX2Rpcm5hbWUsICdzcmMvaG9va3MudHMnKSxcblx0XHRcdFx0d3M6IHJlc29sdmUoX19kaXJuYW1lLCAnc3JjL3dzL2luZGV4LnRzJyksXG5cdFx0XHRcdCd3cy1ydXN0JzogcmVzb2x2ZShfX2Rpcm5hbWUsICdzcmMvd3MtcnVzdC9pbmRleC50cycpXG5cdFx0XHR9LFxuXHRcdFx0b3V0cHV0OiB7XG5cdFx0XHRcdGVudHJ5RmlsZU5hbWVzOiAoY2h1bmtJbmZvKSA9PiB7XG5cdFx0XHRcdFx0Ly8gTWFwIGVudHJ5IG5hbWVzIHRvIGRlc2lyZWQgb3V0cHV0IGZpbGVuYW1lc1xuXHRcdFx0XHRcdGNvbnN0IGVudHJ5TmFtZU1hcDogUmVjb3JkPHN0cmluZywgc3RyaW5nPiA9IHtcblx0XHRcdFx0XHRcdGluZGV4OiAnaW5kZXguanMnLFxuXHRcdFx0XHRcdFx0dXRpbHM6ICd1dGlscy5qcycsXG5cdFx0XHRcdFx0XHRob29rczogJ2hvb2tzLmpzJyxcblx0XHRcdFx0XHRcdHdzOiAnd3MvaW5kZXguanMnLFxuXHRcdFx0XHRcdFx0J3dzLXJ1c3QnOiAnd3MtcnVzdC9pbmRleC5qcydcblx0XHRcdFx0XHR9O1xuXHRcdFx0XHRcdHJldHVybiBlbnRyeU5hbWVNYXBbY2h1bmtJbmZvLm5hbWUgYXMgc3RyaW5nXSB8fCAnW25hbWVdLmpzJztcblx0XHRcdFx0fSxcblx0XHRcdFx0Ly8gSGFuZGxlIC5kLnRzIGZpbGVzXG5cdFx0XHRcdGNodW5rRmlsZU5hbWVzOiAoY2h1bmtJbmZvKSA9PiB7XG5cdFx0XHRcdFx0cmV0dXJuICdbbmFtZV0uanMnO1xuXHRcdFx0XHR9LFxuXHRcdFx0XHQvLyBFbnN1cmUgd29ya2VyIGFuZCBXQVNNIGZpbGVzIGFyZSBwcm9wZXJseSBoYW5kbGVkXG5cdFx0XHRcdGFzc2V0RmlsZU5hbWVzOiAoYXNzZXRJbmZvKSA9PiB7XG5cdFx0XHRcdFx0aWYgKGFzc2V0SW5mby5uYW1lPy5lbmRzV2l0aCgnLndhc20nKSkge1xuXHRcdFx0XHRcdFx0cmV0dXJuICd3YXNtL1tuYW1lXVtleHRuYW1lXSc7XG5cdFx0XHRcdFx0fVxuXHRcdFx0XHRcdGlmIChhc3NldEluZm8ubmFtZT8uaW5jbHVkZXMoJ3dvcmtlcicpKSB7XG5cdFx0XHRcdFx0XHRyZXR1cm4gJ3dvcmtlcnMvW25hbWVdW2V4dG5hbWVdJztcblx0XHRcdFx0XHR9XG5cdFx0XHRcdFx0cmV0dXJuICdhc3NldHMvW25hbWVdW2V4dG5hbWVdJztcblx0XHRcdFx0fVxuXHRcdFx0fVxuXHRcdH0sXG5cdFx0dGFyZ2V0OiAnZXMyMDIyJyxcblx0XHRtaW5pZnk6ICdlc2J1aWxkJyxcblx0XHRzb3VyY2VtYXA6IHRydWUsXG5cdFx0Ly8gUHJldmVudCBpbmxpbmluZyBvZiBhc3NldHMgLSB0aGlzIGlzIGtleSFcblx0XHRhc3NldHNJbmxpbmVMaW1pdDogMCAvLyBUaGlzIHByZXZlbnRzIGJhc2U2NCBpbmxpbmluZ1xuXHR9LFxuXHR3b3JrZXI6IHtcblx0XHRmb3JtYXQ6ICdlcycsXG5cdFx0cm9sbHVwT3B0aW9uczoge1xuXHRcdFx0ZXh0ZXJuYWw6IFsnQGNhbmR5cG9ldHMvcnVzdC13b3JrZXInLCAnQGNhbmR5cG9ldHMvcnVzdC13b3JrZXIvd29ya2VyLmpzJ11cblx0XHR9XG5cdH1cbn0pO1xuIl0sCiAgIm1hcHBpbmdzIjogIjtBQUF5VyxTQUFTLG9CQUFvQjtBQUN0WSxTQUFTLGVBQWU7QUFDeEIsT0FBTyxTQUFTO0FBQ2hCLE9BQU8sVUFBVTtBQUNqQixPQUFPLG1CQUFtQjtBQUoxQixJQUFNLG1DQUFtQztBQU16QyxJQUFPLHNCQUFRLGFBQWE7QUFBQSxFQUMzQixTQUFTO0FBQUEsSUFDUixLQUFLO0FBQUEsSUFDTCxjQUFjO0FBQUEsSUFDZCxJQUFJO0FBQUEsTUFDSCxTQUFTLENBQUMsVUFBVTtBQUFBLE1BQ3BCLFNBQVMsQ0FBQyxtQkFBbUIsaUJBQWlCO0FBQUEsTUFDOUMsUUFBUTtBQUFBLE1BQ1Isa0JBQWtCO0FBQUEsTUFDbEIsV0FBVztBQUFBLE1BQ1gsYUFBYTtBQUFBLE1BQ2IsY0FBYztBQUFBLE1BQ2QsZ0JBQWdCO0FBQUEsSUFDakIsQ0FBQztBQUFBLEVBQ0Y7QUFBQSxFQUNBLFNBQVM7QUFBQSxJQUNSLE9BQU87QUFBQSxNQUNOLEtBQUssUUFBUSxrQ0FBVyxLQUFLO0FBQUEsSUFDOUI7QUFBQSxFQUNEO0FBQUEsRUFDQSxPQUFPO0FBQUEsSUFDTixLQUFLO0FBQUEsTUFDSixPQUFPLFFBQVEsa0NBQVcsY0FBYztBQUFBLE1BQ3hDLE1BQU07QUFBQSxNQUNOLFNBQVMsQ0FBQyxJQUFJO0FBQUEsTUFDZCxVQUFVO0FBQUEsSUFDWDtBQUFBLElBQ0EsZUFBZTtBQUFBLE1BQ2QsVUFBVSxDQUFDLE9BQU87QUFFakIsWUFBSSxHQUFHLFNBQVMseUJBQXlCLEdBQUc7QUFDM0MsaUJBQU87QUFBQSxRQUNSO0FBQ0EsZUFBTyxDQUFDLGVBQWUsYUFBYSxFQUFFLFNBQVMsRUFBRTtBQUFBLE1BQ2xEO0FBQUEsTUFDQSxPQUFPO0FBQUEsUUFDTixPQUFPLFFBQVEsa0NBQVcsY0FBYztBQUFBO0FBQUEsUUFFeEMsT0FBTyxRQUFRLGtDQUFXLGNBQWM7QUFBQSxRQUN4QyxPQUFPLFFBQVEsa0NBQVcsY0FBYztBQUFBLFFBQ3hDLElBQUksUUFBUSxrQ0FBVyxpQkFBaUI7QUFBQSxRQUN4QyxXQUFXLFFBQVEsa0NBQVcsc0JBQXNCO0FBQUEsTUFDckQ7QUFBQSxNQUNBLFFBQVE7QUFBQSxRQUNQLGdCQUFnQixDQUFDLGNBQWM7QUFFOUIsZ0JBQU0sZUFBdUM7QUFBQSxZQUM1QyxPQUFPO0FBQUEsWUFDUCxPQUFPO0FBQUEsWUFDUCxPQUFPO0FBQUEsWUFDUCxJQUFJO0FBQUEsWUFDSixXQUFXO0FBQUEsVUFDWjtBQUNBLGlCQUFPLGFBQWEsVUFBVSxJQUFjLEtBQUs7QUFBQSxRQUNsRDtBQUFBO0FBQUEsUUFFQSxnQkFBZ0IsQ0FBQyxjQUFjO0FBQzlCLGlCQUFPO0FBQUEsUUFDUjtBQUFBO0FBQUEsUUFFQSxnQkFBZ0IsQ0FBQyxjQUFjO0FBQzlCLGNBQUksVUFBVSxNQUFNLFNBQVMsT0FBTyxHQUFHO0FBQ3RDLG1CQUFPO0FBQUEsVUFDUjtBQUNBLGNBQUksVUFBVSxNQUFNLFNBQVMsUUFBUSxHQUFHO0FBQ3ZDLG1CQUFPO0FBQUEsVUFDUjtBQUNBLGlCQUFPO0FBQUEsUUFDUjtBQUFBLE1BQ0Q7QUFBQSxJQUNEO0FBQUEsSUFDQSxRQUFRO0FBQUEsSUFDUixRQUFRO0FBQUEsSUFDUixXQUFXO0FBQUE7QUFBQSxJQUVYLG1CQUFtQjtBQUFBO0FBQUEsRUFDcEI7QUFBQSxFQUNBLFFBQVE7QUFBQSxJQUNQLFFBQVE7QUFBQSxJQUNSLGVBQWU7QUFBQSxNQUNkLFVBQVUsQ0FBQywyQkFBMkIsbUNBQW1DO0FBQUEsSUFDMUU7QUFBQSxFQUNEO0FBQ0QsQ0FBQzsiLAogICJuYW1lcyI6IFtdCn0K
