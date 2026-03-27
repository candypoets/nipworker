# @candypoets/nipworker

A high-performance Nostr client library that moves everything off the main thread and becomes your entire application state layer.

[![npm version](https://badge.fury.io/js/@candypoets%2Fnipworker.svg)](https://badge.fury.io/js/@candypoets%2Fnipworker)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## What is NIPWorker?

Big, opinionated, and built for speed. FlatBuffers and Web Workers at its core, compiled from Rust to WASM.

## Framework Agnostic

Works with any frontend. No React dependency, no Svelte stores to learn, no Vue composables. Just hook-like methods with callbacks that you wire to your framework's reactivity however you want.

## 4 Dedicated Workers

• **Connections** — Relay connections, WebSocket lifecycle, reconnection backoff. Owns all network I/O.

• **Cache** — Stores FlatBuffers in ring buffers in real time and IndexedDB in timeout chunks. No refetching what you already have.

• **Parser** — Event validation, signature verification, content parsing. Receives raw JSON from relays, outputs FlatBuffers to your frontend. No JSON.parse on the main thread. Ever.

• **Crypto** — Signing, NIP:04/44 encryption, NIP:46 remote signer sessions, Cashu proof verification.

Each worker runs in its own Web Worker. The main thread just orchestrates. Heavy work happens in parallel.

## FlatBuffers Instead of JSON

NIPWorker speaks FlatBuffers end to end. Raw relay messages get parsed once in Rust, then flow through the system as zero-copy binary views. No JSON.parse. No object allocation. No GC pauses on infinite scroll.

Your components read directly from FlatBuffers tables. A Kind1 note's content blocks (images, videos, hashtags) arrive pre-parsed. You iterate them with `fbArray()` and render straight from the binary buffer to the DOM. The schema lives from wire to HTML.

## Apollo-Inspired State Management

Like Apollo Client, NIPWorker IS your store. You do not need Redux, Zustand, or custom state libraries.

`useSubscribe` pulls FlatBuffers from the worker pool and feeds your UI directly. Subscriptions accept fetch policies: `cacheFirst` serves from memory immediately if available, `noCache` always bypass the cache and hits the network. You control the speed versus freshness tradeoff per query.

`usePublish` sends events and tracks relay acknowledgments. Your entire app state flows through these two hooks. Subscriptions are deduped across components automatically. The library manages the cache, merge logic, and reactive updates.

## Pipeline Architecture

Events flow through a processing pipeline: verify → dedupe → filter → transform → store. Each subscription configures its own pipeline. The pipeline runs in the Parser worker before FlatBuffers reach your callback.

## Opinionated by Design

NIPWorker enforces outbox model by default. It reads every author's NIP:65 relay list to discover where they publish. The library manages relay discovery and publication strategy for you.

Built for clients that need to render thousands of events without dropping frames.

## Installation

```bash
npm install @candypoets/nipworker
```

Install the skill for AI assistance:

```bash
npx skills add candypoets/skills@nipworker
```

## Quick Start

```typescript
import { createNostrManager, setManager } from '@candypoets/nipworker';
import { useSubscription, usePublish } from '@candypoets/nipworker/hooks';
import { isKind1, asKind1, fbArray } from '@candypoets/nipworker/utils';

// Create and set the global manager
const manager = createNostrManager();
setManager(manager);

// Subscribe to events
const unsubscribe = useSubscription(
  'feed_home',
  [{ kinds: [1], limit: 50, relays: ['wss://relay.example.com'] }],
  (msg) => {
    const kind1 = isKind1(msg);
    if (kind1) {
      // Access content blocks directly from FlatBuffers view
      const blocks = fbArray(kind1, 'contentBlocks');
      renderNote(blocks);
    }
  }
);
```

## Supported NIPs

| NIP | Description | Status |
|-----|-------------|--------|
| NIP-01 | Basic Protocol | ✅ Full |
| NIP-02 | Contact List | ✅ Full |
| NIP-04 | Encrypted DMs | ✅ Full |
| NIP-18 | Reposts | ✅ Full |
| NIP-19 | bech32 Entities | ✅ Full |
| NIP-25 | Reactions | ✅ Full |
| NIP-44 | Versioned Encryption | ✅ Full |
| NIP-46 | Nostr Connect | ✅ Full |
| NIP-51 | Lists | ✅ Full |
| NIP-57 | Lightning Zaps | ✅ Full |
| NIP-60 | Cashu Wallet | ✅ Full |
| NIP-61 | Nutzaps | ✅ Full |
| NIP-65 | Relay Lists | ✅ Full |

## Documentation

See [AGENTS.md](AGENTS.md) for detailed architecture documentation.

## License

MIT License - see [LICENSE](LICENSE) for details.

---

Made with ❤️ by [So Tachi](mailto:sotachi@proton.me)
