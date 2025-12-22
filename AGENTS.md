# NIPWorker: Agent Guidelines

## Commands

- **Build All**: `npm run build` (Rust WASM + TypeScript bundle)
- **Crates**: `npm run build:parser|cache|connections|signer` (wasm-pack)
- **FlatBuffers**: `npm run flatc` (Regenerate TS/Rust types from `/schemas`)
- **Types**: `npm run build:types` (Generate .d.ts files)
- **Test**: No standard script; use `npx vitest` if available.

## Code Style

- **TypeScript**: `camelCase` variables, `PascalCase` types. Explicit returns.
- **Formatting**: Tabs, 100w, single quotes (via `.prettierrc`).
- **Rust**: `snake_case`, `thiserror` for internals, `JSError` for WASM exports.
- **Imports**: External first, then internal project imports, then type-only.

## Architecture: 4 Rust Crates Integration

- **Orchestrator**: `NostrManager` (`src/index.ts`) spawns 4 Web Workers.
- **Workers**: `parser`, `cache`, `connections`, `signer` (Rust WASM).
- **IPC**: SharedArrayBuffer ring buffers using `sab_ring.rs` (`src/shared`).
- **Protocol**: FlatBuffers (binary) for zero-copy cross-thread communication.
- **Flow**: TS Manager ↔ `sab_ring` ↔ WASM Crate ↔ `sab_ring` ↔ TS.

## Development Rules

- **Schema**: Run `npm run flatc` immediately after any `.fbs` modification.
- **IPC Safety**: Header layout in `sab_ring.rs` must remain 32-byte consistent.
- **Performance**: Use `SharedBufferReader` class; favor cache over network.
- **Casing**: TS imports MUST match file casing exactly (e.g., `NostrUtils.ts`).
