# Nostr Event Parser Module

This module provides comprehensive parsing functionality for Nostr events, supporting all major event kinds used in the NutsCash application.

## Overview

The parser module is a complete Rust rewrite of the original Go parser, maintaining full compatibility while providing better type safety and performance. It handles parsing, validation, and preparation of Nostr events across various kinds.

## Supported Event Kinds

### Core Nostr Events
- **Kind 0**: Profile metadata events (NIP-01)
- **Kind 1**: Text note events (NIP-01)
- **Kind 3**: Contact list events (NIP-02)
- **Kind 4**: Encrypted direct message events (NIP-04)
- **Kind 6**: Repost events (NIP-18)
- **Kind 7**: Reaction events (NIP-25)
- **Kind 17**: Website reaction events
- **Kind 9735**: Zap receipt events (NIP-57)
- **Kind 10002**: Relay list metadata events (NIP-65)
- **Kind 39089**: Categorized people list events (NIP-51)

### NutsCash-Specific Events
- **Kind 7374**: Quote events for Cashu token redemption
- **Kind 7375**: Token events containing Cashu proofs
- **Kind 7376**: Spending history events
- **Kind 9321**: Nutzap events (NIP-61)
- **Kind 10019**: NutsCash user settings
- **Kind 17375**: Encrypted wallet events

## Architecture

### Parser Structure

```rust
pub struct Parser {
    pub default_relays: Vec<String>,
    pub indexer_relays: Vec<String>,
    pub relay_hints: HashMap<String, Vec<String>>,
}
```

### Main Methods

- `parse(event: Event) -> Result<ParsedEvent>` - Main parsing entry point
- `prepare(event: &mut Event) -> Result<()>` - Prepares events for publishing
- `get_relays(kind: u64, pubkey: &str, write: Option<bool>) -> Vec<String>` - Gets relay hints
- `get_relay_hint(event: &Event) -> Vec<String>` - Extracts relay hints from events

## Content Parsing

The module includes sophisticated content parsing capabilities that handle:

- **Rich text formatting**: Code blocks, hashtags, mentions
- **Media content**: Images, videos with automatic grouping
- **Nostr entities**: npub, note, nevent, nprofile references (NIP-19)
- **Links**: Automatic link detection with preview placeholders
- **Cashu tokens**: Recognition and parsing of Cashu token strings

### Content Block Types

```rust
pub struct ContentBlock {
    pub block_type: String,    // "text", "hashtag", "image", "link", etc.
    pub text: String,          // Original text
    pub data: Option<Value>,   // Parsed data specific to block type
}
```

## Event-Specific Parsing

Each event kind has its own dedicated parser module:

### Profile Events (Kind 0)
- Handles multiple field formats (name/displayName, about/bio, etc.)
- NIP-05 verification support (async)
- Fallback logic for missing fields

### Text Notes (Kind 1)
- Content parsing into structured blocks
- NIP-10 threading support (reply/root detection)
- NIP-27 entity reference parsing
- Automatic profile and relay list requests

### Contact Lists (Kind 3)
- Structured contact extraction
- Relay hints and petnames
- Deduplication

### Direct Messages (Kind 4)
- NIP-04 encryption/decryption support
- Chat ID generation
- Content parsing for decrypted messages

### Reactions (Kind 7)
- Multiple reaction types (+, -, emoji, custom)
- Emoji tag parsing
- Target event resolution

### Relay Lists (Kind 10002)
- Read/write marker support
- URL normalization
- Deduplication

## Encryption Support

The parser supports NIP-04 and NIP-44 encryption for relevant event kinds:

- **NIP-04**: Used for direct messages (Kind 4)
- **NIP-44**: Used for NutsCash-specific encrypted events

Note: Actual encryption/decryption requires a signer implementation.

## Request Generation

The parser automatically generates follow-up requests for related data:

- Profile information for mentioned users
- Relay lists for event authors
- Referenced events
- Related NutsCash events (spending history, etc.)

## Error Handling

The parser uses Rust's `Result` type for comprehensive error handling:

```rust
pub enum ParseError {
    InvalidEventKind,
    MissingRequiredTags,
    InvalidContent,
    EncryptionRequired,
    // ... other error types
}
```

## Testing

Comprehensive test suite covers:

- All supported event kinds
- Content parsing edge cases
- Relay hint extraction
- Error conditions
- Integration scenarios

Run tests with:
```bash
cargo test parser::tests
```

## Usage Example

```rust
use nutscash_nostr::parser::Parser;

// Initialize parser
let default_relays = vec!["wss://relay.damus.io".to_string()];
let indexer_relays = vec!["wss://relay.nostr.band".to_string()];
let mut parser = Parser::new(default_relays, indexer_relays);

// Parse an event
let parsed_event = parser.parse(event)?;

// Access parsed data
if let Some(parsed_data) = parsed_event.parsed {
    println!("Parsed: {}", parsed_data);
}

// Handle follow-up requests
if let Some(requests) = parsed_event.requests {
    for request in requests {
        // Process each request...
    }
}
```

## Migration from Go

Key differences from the original Go implementation:

1. **Type Safety**: Strict typing with Rust's type system
2. **Error Handling**: Comprehensive Result-based error handling
3. **Memory Safety**: No risk of memory leaks or buffer overflows
4. **Performance**: Zero-cost abstractions and efficient parsing
5. **Testing**: More comprehensive test coverage

## Future Enhancements

- [ ] Signer integration for encryption/decryption
- [ ] Database integration for relay list queries
- [ ] Async support for NIP-05 verification
- [ ] Additional Cashu token validation
- [ ] Performance optimizations
- [ ] WebAssembly bindings

## Dependencies

- `nostr`: Core Nostr types and utilities
- `serde`: Serialization/deserialization
- `regex`: Pattern matching for content parsing
- `anyhow`: Error handling
- `url`: URL parsing and validation

## Contributing

When adding support for new event kinds:

1. Create a new module file (e.g., `kind{number}.rs`)
2. Implement the parsing logic
3. Add the kind to the main parser match statement
4. Include comprehensive tests
5. Update this README

All parsers should follow the established patterns:
- Return `Result<(ParsedType, Option<Vec<Request>>)>`
- Handle validation and error cases
- Generate appropriate follow-up requests
- Include thorough testing