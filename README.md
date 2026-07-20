# @candypoets/nipworker

NIPWorker is one Nostr SDK across platforms: web, React Native, native integrations, and relay proxy
tooling. It gives applications the same core model for subscribing, publishing, caching, parsing,
signing, and tracking relay state without making every app rebuild the protocol layer.

It is highly optimized and opinionated, but still lightweight at the API surface. NIPWorker pushes
relay I/O, parsing, cache work, and signing into Rust-backed workers or native engines, then exposes
small TypeScript entry points, framework-agnostic callback helpers, generated FlatBuffers views, and
signer integration.

It is built for Nostr clients with real feeds and long-lived sessions: social apps, wallets,
messaging clients, media browsers, dashboards, and any interface that has to keep many relay
connections active while rendering quickly. If your app only needs to fetch a handful of events,
`nostr-tools` may be enough. If your app needs a fast reusable Nostr runtime across platforms,
NIPWorker is meant for that layer.

[![npm version](https://badge.fury.io/js/@candypoets%2Fnipworker.svg)](https://badge.fury.io/js/@candypoets%2Fnipworker)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## Platforms

| Platform | Support | Entry point |
| --- | --- | --- |
| Web apps | Supported with Web Workers and Rust WASM. | `@candypoets/nipworker` |
| Vite apps | Supported; proxy helpers are available for local or server-side relay routing. | `@candypoets/nipworker`, `@candypoets/nipworker/proxy/vite` |
| React Native | Supported through the native backend. | `@candypoets/nipworker/react-native` |
| Node.js servers | Supported for relay proxy server utilities; the main client runtime targets app environments. | `@candypoets/nipworker/proxy/server` |
| Swift/native integrations | Native artifacts and Swift package sources are included in this repository. | `swift/` package sources |

The browser API is framework-agnostic. It can be used from React, Svelte, Vue, Solid, vanilla
TypeScript, or any environment that can run module workers and WASM.

## Install

```bash
npm install @candypoets/nipworker flatbuffers
```

Optional peer dependencies:

- `vite` for browser builds and the Vite relay proxy plugin.
- `ws` for the relay proxy server.
- `react-native` for the React Native native backend.

## Quick Start

```ts
import { createNostrManager, setManager } from '@candypoets/nipworker';
import { useSubscription, usePublish, isKind1 } from '@candypoets/nipworker/hooks';
import { fbArray } from '@candypoets/nipworker/utils';

const manager = createNostrManager({
	defaultRelays: ['wss://relay.damus.io', 'wss://nos.lol'],
	indexerRelays: ['wss://purplepag.es']
});

setManager(manager);

const stop = useSubscription(
	'home-feed',
	[{ kinds: [1], limit: 50, relays: ['wss://relay.damus.io'] }],
	(message) => {
		const note = isKind1(message);
		if (!note) return;

		const blocks = fbArray(note, 'contentBlocks');
		renderNote(note, blocks);
	},
	{ cacheFirst: true, closeOnEose: false }
);

const stopPublish = usePublish(
	'publish-1',
	{ kind: 1, content: 'hello nostr', tags: [] },
	(message) => {
		console.log(message);
	},
	{ defaultRelays: ['wss://relay.damus.io'] }
);
```

The `use*` functions are framework-agnostic callback helpers. They do not depend on React and can be
adapted to React, Svelte, Vue, Solid, or plain TypeScript state.

## Backends

### Browser, Multi-Worker

`createNostrManager()` creates the default browser backend, `NostrManager`. It starts dedicated
workers for:

- `connections`: WebSocket relay connections and relay lifecycle.
- `cache`: cached event storage and cache lookups.
- `parser`: validation, filtering, pipelines, and FlatBuffers output.
- `crypto`: signing, NIP-04/NIP-44 operations, NIP-46, and proof verification.

Workers communicate with `MessageChannel` ports and FlatBuffers messages.

### React Native

React Native must import from the native entry point. This path avoids browser WASM worker imports
and talks to the native module instead.

```ts
import { createNostrManager, setManager } from '@candypoets/nipworker/react-native';

setManager(
	createNostrManager({
		defaultRelays: ['wss://relay.damus.io'],
		indexerRelays: ['wss://purplepag.es']
	})
);
```

## Public Entry Points

| Export | Purpose |
| --- | --- |
| `@candypoets/nipworker` | Manager factories, manager classes, types, generated FlatBuffers exports. |
| `@candypoets/nipworker/hooks` | Callback helpers for subscriptions, publishing, signing, and relay status. |
| `@candypoets/nipworker/utils` | FlatBuffers helpers, type guards, content parsing, NIP-46 QR helper. |
| `@candypoets/nipworker/proxy` | Browser proxy client. |
| `@candypoets/nipworker/proxy/server` | Node relay proxy server. |
| `@candypoets/nipworker/proxy/vite` | Vite plugin for relay proxy integration. |
| `@candypoets/nipworker/react-native` | React Native native backend. |
| `@candypoets/nipworker/legacy` | Legacy compatibility entry point. |

## Signers

The manager supports several signer modes:

```ts
manager.setSigner('privkey', '<hex-secret-key>');
manager.setNip07();
manager.setNip46Bunker('<bunker-url>');
manager.setNip46QR('<nostrconnect-url>');
manager.setPubkey('<readonly-pubkey>');
```

`useSignEvent(template, callback)` signs through the active signer.

## Subscription Options

Subscriptions accept relay requests plus options for cache behavior, lifecycle, and pipeline control:

```ts
useSubscription(
	'profile',
	[{ kinds: [0], authors: [pubkey], relays: ['wss://purplepag.es'], limit: 1 }],
	onMessage,
	{
		cacheFirst: true,
		cacheOnly: false,
		closeOnEose: true,
		timeoutMs: 5000,
		bytesPerEvent: 4096
	}
);
```

Useful request flags include `cacheFirst`, `noCache`, `closeOnEOSE`, `count`, `maxRelays`, and
`noOptimize`.

## FlatBuffers and Data Movement

NIPWorker uses FlatBuffers for worker messages and parsed event data. Components can read directly
from generated FlatBuffers views and use helpers such as `fbArray()` and `fbIterable()` for vector
fields.

Within JavaScript worker-to-worker paths, buffers are transferred with `postMessage(data,
[transferable])`. Across the JS/WASM boundary, bytes still have to be copied into or out of WASM
linear memory. FlatBuffers parsing itself avoids object deserialization and allocation-heavy JSON
work, but this is not strict end-to-end zero-copy across WASM.

## Development

Prerequisites:

- Node.js 18+
- Rust 1.70+
- `wasm-pack`
- `flatc` for schema generation

Common commands:

```bash
npm run build          # Build WASM crates, native artifacts, and TypeScript bundle
npm run build:crates   # Build browser WASM crates only
npm run build:native   # Build Android and iOS native artifacts
npm run build:types    # Emit TypeScript declarations
npm test               # Run unit tests
npm run test:e2e       # Run Playwright tests
```

Schema generation:

```bash
npm run flatc          # Rust, TypeScript, and Java generated files
npm run flatc:rust
npm run flatc:ts
npm run flatc:java
npm run flatc:swift
```

When editing files in `schemas/`, regenerate the affected FlatBuffers outputs before committing.

## Repository Layout

| Path | Purpose |
| --- | --- |
| `src/NostrManager.ts` | Default browser multi-worker manager. |
| `src/react-native.ts` | React Native manager and native bridge. |
| `src/hooks.ts` | Framework-agnostic callback helpers. |
| `src/types/index.ts` | Public TypeScript types. |
| `src/lib/` | Shared TypeScript helpers. |
| `src/generated/` | Generated TypeScript FlatBuffers code. |
| `crates/connections/` | Browser relay connection WASM crate. |
| `crates/cache/` | Browser cache WASM crate. |
| `crates/parser/` | Browser parser WASM crate. |
| `crates/crypto/` | Browser crypto WASM crate. |
| `crates/core/` | Shared Rust core/generated code. |
| `crates/native-ffi/` | Native FFI and React Native platform bindings. |
| `schemas/` | FlatBuffers schemas. |
| `swift/` | Swift package for native integration. |

## Supported NIPs

The generated schema and parser currently include support for common Nostr event kinds across
NIP-01, NIP-02, NIP-04, NIP-18, NIP-19, NIP-25, NIP-44, NIP-46, NIP-51, NIP-57, NIP-60, NIP-61, and
NIP-65, plus newer parsed kinds such as long-form articles, media, polls, live activities, and
community/group-related events.

## License

MIT License - see [LICENSE](LICENSE) for details.
