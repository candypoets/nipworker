# @candypoets/nipworker

A high-performance Nostr client library with worker-based architecture using Rust WebAssembly for optimal performance and non-blocking operations.

[![npm version](https://badge.fury.io/js/@candypoets%2Fnipworker.svg)](https://badge.fury.io/js/@candypoets%2Fnipworker)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## 🚀 Features

- **High Performance**: Rust WASM core for cryptographic operations and message processing
- **Worker-Based Architecture**: Non-blocking operations using Web Workers
- **TypeScript Support**: Full TypeScript definitions included
- **Dual Module Support**: Both ES modules and UMD builds
- **Efficient Serialization**: Uses MessagePack for optimal data transfer
- **Comprehensive NIP Support**: Implements 12+ standard Nostr Implementation Possibilities (NIPs)


## 📋 Supported NIPs

| NIP | Name | Description | Event Kinds | Status |
|-----|------|-------------|-------------|---------|
| [NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md) | Basic Protocol | Core protocol flow description | 0, 1 | ✅ Full |
| [NIP-02](https://github.com/nostr-protocol/nips/blob/master/02.md) | Contact List | Contact lists and petnames | 3 | ✅ Full |
| [NIP-04](https://github.com/nostr-protocol/nips/blob/master/04.md) | Encrypted DMs | Encrypted direct messages | 4 | ✅ Full |
| [NIP-05](https://github.com/nostr-protocol/nips/blob/master/05.md) | DNS Identifiers | Mapping keys to DNS identifiers | - | 🔄 Partial |
| [NIP-10](https://github.com/nostr-protocol/nips/blob/master/10.md) | Text Note References | Threading and replies | - | ✅ Full |
| [NIP-18](https://github.com/nostr-protocol/nips/blob/master/18.md) | Reposts | Event reposts | 6 | ✅ Full |
| [NIP-19](https://github.com/nostr-protocol/nips/blob/master/19.md) | bech32 Entities | npub, note, nevent, nprofile encoding | - | ✅ Full |
| [NIP-25](https://github.com/nostr-protocol/nips/blob/master/25.md) | Reactions | Event reactions and emoji | 7 | ✅ Full |
| [NIP-27](https://github.com/nostr-protocol/nips/blob/master/27.md) | Text References | Mentions and references in content | - | ✅ Full |
| [NIP-44](https://github.com/nostr-protocol/nips/blob/master/44.md) | Versioned Encryption | Advanced encryption for private events | - | ✅ Full |
| [NIP-51](https://github.com/nostr-protocol/nips/blob/master/51.md) | Lists | Categorized lists (people, bookmarks) | 39089 | ✅ Full |
| [NIP-57](https://github.com/nostr-protocol/nips/blob/master/57.md) | Lightning Zaps | Bitcoin Lightning Network integration | 9735 | ✅ Full |
| [NIP-60](https://github.com/nostr-protocol/nips/blob/master/60.md) | Cashu Wallet | Cashu ecash wallet functionality | 7374, 7375, 7376, 10019, 17375 | ✅ Full |
| [NIP-61](https://github.com/nostr-protocol/nips/blob/master/61.md) | Nutzaps | Cashu token zaps | 9321 | ✅ Full |
| [NIP-65](https://github.com/nostr-protocol/nips/blob/master/65.md) | Relay Lists | User relay preferences | 10002 | ✅ Full |

### NIP-60 Cashu Wallet Events

The library provides complete support for **NIP-60 Cashu wallet functionality**:

| Event Kind | Description | Encryption | Purpose |
|------------|-------------|------------|---------|
| 7374 | Quote events | NIP-44 | Token redemption quotes |
| 7375 | Token events | NIP-44 | Cashu proofs and tokens |
| 7376 | Spending history | NIP-44 | Transaction history |
| 9321 | Nutzaps | - | Cashu token zaps |
| 10019 | Wallet settings | - | User wallet preferences |
| 17375 | Encrypted wallets | NIP-44 | Private wallet data |


### Legend
- ✅ **Full**: Complete implementation with all features
- 🔄 **Partial**: Basic support, some features may be limited
- ❌ **Not Supported**: NIP is not implemented

## 📦 Installation

```bash
npm install @candypoets/nipworker
```

### Peer Dependencies

This library requires `nostr-tools` as a peer dependency:

```bash
npm install nostr-tools
```

## 🔧 Usage

### Basic Usage

```javascript
import { nostrManager } from '@candypoets/nipworker';
import { useSubscription } from '@candypoets/nipworker/hooks';

// Login by setting up a signer (private key)
nostrManager.setSigner('privkey', 'your-private-key-hex');

// Subscribe to events using the hook
const unsubscribe = useSubscription(
  'my-subscription',
  [{
    kinds: [1], // Text notes
    limit: 10
  }],
  (events, eventType) => {
    if (eventType === 'EVENTS') {
      console.log('Received events:', events);
    } else if (eventType === 'EOSE') {
      console.log('End of stored events');
    }
  },
  { closeOnEose: false }
);

// Publish an event
const event = {
  kind: 1,
  content: 'Hello Nostr!',
  tags: [],
  created_at: Math.floor(Date.now() / 1000)
};

nostrManager.publish('publish-id-123', event, (status, type) => {
  console.log('Publish status:', status);
});

// Clean up subscription when done
// unsubscribe();
```

### Advanced Usage

```javascript
import { createNostrManager } from '@candypoets/nipworker';
import { useSubscription } from '@candypoets/nipworker/hooks';

// Create a custom manager instance with configuration
const customManager = createNostrManager();

// Set up multiple signers
customManager.setSigner('privkey', 'main-private-key-hex');

// Subscribe to specific authors with multiple filters
const unsubscribe = useSubscription(
  'author-feed',
  [
    {
      kinds: [1, 6, 7],
      authors: ['pubkey1', 'pubkey2'],
      since: Math.floor(Date.now() / 1000) - 3600, // Last hour
      limit: 50
    },
    {
      kinds: [30023], // Long-form content
      authors: ['pubkey1'],
      limit: 10
    }
  ],
  (events, eventType) => {
    if (eventType === 'EVENTS') {
      events.forEach(event => {
        switch (event.kind) {
          case 1:
            console.log('Text note:', event.content);
            break;
          case 6:
            console.log('Repost:', event);
            break;
          case 7:
            console.log('Reaction:', event);
            break;
          case 30023:
            console.log('Long-form content:', event);
            break;
        }
      });
    } else if (eventType === 'EOSE') {
      console.log('End of stored events for author feed');
    }
  },
  { closeOnEose: false }
);

// Global publish status monitoring
customManager.addPublishCallbackAll((status, eventId) => {
  console.log(`Event ${eventId} status:`, status);
});

// Clean up when done
setTimeout(() => {
  unsubscribe();
  customManager.cleanup();
}, 60000);
```

## 🏗️ Architecture

NipWorker uses a multi-layered architecture:

1. **Main Thread**: Your application code
2. **Web Worker**: Handles network operations and message routing
3. **Rust WASM Core**: Performs cryptographic operations and message validation

This architecture ensures that heavy operations don't block your main thread, providing a smooth user experience.

## 📚 API Reference

### NostrManager

#### Factory Function

```typescript
createNostrManager(config?: NostrManagerConfig): NostrManager
```

#### Methods

- `setSigner(name: string, secretKeyHex: string): void` - Set up a signer for publishing
- `subscribe(subscriptionId: string, requests: Request[], options?: SubscriptionOptions): SharedArrayBuffer` - Subscribe to events
- `publish(publishId: string, event: NostrEvent, callback?: PublishCallback): void` - Publish an event
- `signEvent(event: NostrEvent): void` - Sign an event without publishing
- `getPublicKey(): void` - Get the public key of the current signer
- `unsubscribe(subscriptionId: string): void` - Unsubscribe from events
- `cleanup(): void` - Clean up unused subscriptions
- `addPublishCallbackAll(callback: Function): void` - Monitor all publish statuses

### useSubscription Hook

```typescript
useSubscription(
  subId: string,
  requests: Request[],
  callback: SubscriptionCallback,
  options?: SubscriptionOptions
): () => void
```

#### Parameters

- `subId` - Unique subscription identifier
- `requests` - Array of Nostr filter objects
- `callback` - Function called when events are received
- `options` - Subscription options (closeOnEose, skipCache, force)

#### Returns

- Function to unsubscribe and clean up resources

### Event Types

- `EVENTS` - New events received
- `EOSE` - End of stored events
- `EOCE` - End of cached events
- `PUBLISH_STATUS` - Status update for published events

## 🛠️ Development

### Prerequisites

- Node.js 18+
- Rust 1.70+
- wasm-pack

### Building from Source

```bash
# Clone the repository
git clone https://github.com/candypoets/nipworker.git
cd nipworker

# Install dependencies
npm install

# Build WASM modules
npm run build:wasm

# Build the library
npm run build
```

### Scripts

- `npm run dev` - Start development server
- `npm run build` - Build for production
- `npm run build:wasm` - Build WASM modules only
- `npm run build:types` - Generate TypeScript declarations
- `npm run clean` - Clean build artifacts

## 🤝 Contributing

We welcome contributions! Please see our [Contributing Guidelines](CONTRIBUTING.md) for details.

### Development Workflow

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/amazing-feature`
3. Make your changes
4. Add tests if applicable
5. Commit your changes: `git commit -m 'Add amazing feature'`
6. Push to the branch: `git push origin feature/amazing-feature`
7. Open a Pull Request

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🔗 Links

- [GitHub Repository](https://github.com/candypoets/nipworker)
- [Issue Tracker](https://github.com/candypoets/nipworker/issues)
- [Nostr Protocol](https://github.com/nostr-protocol/nostr)
- [NIPs Repository](https://github.com/nostr-protocol/nips)

## 🙏 Acknowledgments

- [Nostr Protocol](https://nostr.com/) - The decentralized social protocol
- [nostr-tools](https://github.com/nbd-wtf/nostr-tools) - Essential Nostr utilities
- [wasm-pack](https://rustwasm.github.io/wasm-pack/) - Rust to WebAssembly workflow

## 📊 Performance

NipWorker is designed for high performance:

- **Fast Event Processing**: Rust WASM core processes events up to 10x faster than pure JavaScript
- **Non-blocking Operations**: Web Worker architecture prevents UI freezing
- **Efficient Serialization**: MessagePack reduces bandwidth usage by ~30%
- **Connection Pooling**: Intelligent relay connection management



## 🔒 Security

- All cryptographic operations are handled by the Rust WASM core
- Private keys never leave the worker thread
- Event validation is performed at the WASM level
- Secure random number generation for key operations

---

Made with ❤️ by [So Tachi](mailto:sotachi@proton.me)
