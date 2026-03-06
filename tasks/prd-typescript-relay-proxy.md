# PRD: TypeScript Relay Proxy

## 1. Introduction

Add a TypeScript relay proxy server for NIPWorker. In proxy mode, the browser client opens one WebSocket to the proxy instead of opening direct relay sockets to many relays.

This PRD is intentionally narrow. It defines the proxy transport contract between client and proxy. It does not redefine parsing, caching, crypto, signer ownership, or broader remote-runtime architecture.

## 2. Goals

- Add a TypeScript WebSocket relay proxy server that owns upstream relay sockets.
- Allow applications to enable proxy-backed relay connections with `proxy?: { url: string }`.
- Reuse existing FlatBuffer request payloads from client to proxy.
- Reuse existing `WorkerMessage` FlatBuffer payloads from proxy to client.
- Define an explicit transport shape for relay `EVENT`, `NOTICE`, `OK`, `CLOSED`, and `AUTH` frames.
- Deduplicate events by `event.id` within subscription scope before forwarding to the client.
- Keep the existing client parser/cache/crypto pipeline intact as much as possible.

## 3. User Stories

### US-001: Connect client to one proxy socket
**Description:** As a client application, I want to open one WebSocket to the proxy so that the browser no longer manages many direct relay sockets.

**Acceptance Criteria:**
- [ ] Client runtime exposes `proxy?: { url: string }` for relay transport.
- [ ] Proxy mode uses exactly one browser-to-proxy WebSocket connection.
- [ ] Local mode remains unchanged.
- [ ] The client can send binary FlatBuffer payloads over the proxy socket.

### US-002: Reuse the existing parser/cache to connections contract
**Description:** As the client runtime, I want proxy mode to preserve the current worker messaging flow so that cache and parser do not need major changes.

**Acceptance Criteria:**
- [ ] Main thread continues sending `MainMessage` bytes to `parser`, not directly to `connections`.
- [ ] `connections` continues consuming the existing bytes it receives from `parser` and `cache`.
- [ ] The cache crate can keep sending its current relay `Envelope` format to `connections`.
- [ ] Proxy mode translation happens inside `connections`, not inside `cache`.
- [ ] Publish result routing uses the event ID, not a `publish_id`.

### US-003: Forward relay messages back as worker-compatible payloads
**Description:** As the client runtime, I want to receive worker-compatible FlatBuffer payloads from the proxy so that the existing parser-side contract stays stable.

**Acceptance Criteria:**
- [ ] Proxy sends binary WebSocket frames to the client.
- [ ] Normal relay `EVENT` traffic is forwarded as `WorkerMessage` with `type = NostrEvent` when possible.
- [ ] Relay status traffic is forwarded as `WorkerMessage` with `type = ConnectionStatus`.
- [ ] `Raw` is used only as a fallback for frames that cannot be expressed with the existing worker message types.

### US-004: Forward AUTH challenges through the existing client path
**Description:** As the client runtime, I want relay AUTH challenges forwarded from the proxy in a stable format so that the existing client transport path can route them correctly.

**Acceptance Criteria:**
- [ ] Relay `AUTH` frames are forwarded from proxy to client as `WorkerMessage` payloads, not ad hoc JSON messages.
- [ ] Phase 1 format for relay `AUTH` is `WorkerMessage::ConnectionStatus` with `status = "AUTH"` and `message = <challenge>`.
- [ ] The forwarded `WorkerMessage` includes the relay URL.
- [ ] The forwarded `WorkerMessage` includes the relevant `sub_id` when available from the transport context.
- [ ] Client `connections` translates incoming AUTH `WorkerMessage` into the existing `SignerRequest(AuthEvent)` message for `crypto`.
- [ ] The `SignerRequest(AuthEvent)` payload remains JSON with `challenge`, `relay`, and `created_at`.
- [ ] `crypto` returns the existing `SignerResponse` shape for AUTH signing results.
- [ ] Client `connections` translates the AUTH signing result into a small JSON proxy command containing `type`, `relay`, and `event`.
- [ ] The PRD does not redefine crypto behavior; it only requires the proxy to preserve enough data for the existing client path to handle AUTH challenges correctly.

### US-005: Deduplicate duplicate relay events within subscription scope
**Description:** As a client application, I want the proxy to suppress duplicate events from multiple relays so that the browser receives less redundant traffic.

**Acceptance Criteria:**
- [ ] Proxy deduplicates by `event.id`.
- [ ] Deduplication scope is the client subscription identified by `Subscribe.subscription_id`.
- [ ] Dedup state is created on `Subscribe` and cleared on `Unsubscribe`, socket disconnect, or session cleanup.
- [ ] The same event may still be forwarded for a different subscription ID.
- [ ] Client-side dedup remains in place.

## 4. Functional Requirements

- FR-1: The repo must expose a TypeScript server entrypoint for running the relay proxy.
- FR-2: The client runtime must expose `proxy?: { url: string }`.
- FR-3: If `proxy` is absent, the client uses local direct-relay mode.
- FR-4: If `proxy` is present, the client uses proxy-backed relay mode and connects to `proxy.url`.
- FR-5: In proxy mode, the browser client must connect to the relay proxy using a single WebSocket.
- FR-6: Main thread command payloads remain serialized `MainMessage` FlatBuffer bytes from [schemas/main.fbs](/root/code/nipworker/schemas/main.fbs), and they continue to flow from main to parser.
- FR-7: The parser and cache workers must continue to communicate with `connections` through their existing contracts in Phase 1.
- FR-8: The cache crate may continue sending its current JSON relay `Envelope` format to `connections` in proxy mode.
- FR-9: Proxy-specific translation from client worker messages to proxy wire messages must happen inside `connections`.
- FR-10: `Subscribe.subscription_id` must be preserved by the proxy and used as the routing key for downstream messages and dedup scope.
- FR-11: Relay selection must remain request-driven through `Request.relays`.
- FR-12: Proxy-to-client payloads must be binary FlatBuffer messages compatible with [schemas/message.fbs](/root/code/nipworker/schemas/message.fbs).
- FR-13: For normal relay `EVENT` frames, the proxy must emit `WorkerMessage` with `type = NostrEvent` when the payload can be represented directly.
- FR-14: For relay `NOTICE`, `OK`, `CLOSED`, and `AUTH` frames, the proxy must emit `WorkerMessage` with `type = ConnectionStatus`.
- FR-15: Phase 1 `AUTH` forwarding must use `ConnectionStatus.status = "AUTH"` and `ConnectionStatus.message = <challenge>`.
- FR-16: The proxy must not emit `Eoce`, because the proxy does not own cache semantics.
- FR-17: `WorkerMessage.sub_id` and `WorkerMessage.url` must be populated on proxy-to-client messages whenever the current schema supports them.
- FR-18: `Raw` messages must be reserved for fallback handling of unsupported or malformed relay frames.
- FR-19: The proxy must open and manage upstream WebSocket connections to the relays requested by the client.
- FR-20: The proxy must deduplicate events by `event.id` within the scope of a single `subscription_id`.
- FR-21: Deduplication must occur before forwarding the event downstream to the client.
- FR-22: Dedup state must be created on `Subscribe` and cleared on `Unsubscribe`, socket disconnect, or session cleanup.
- FR-23: One browser WebSocket connection equals one proxy session for routing and dedup state.
- FR-24: Publish result routing uses the event ID, not a `publish_id`.
- FR-25: Publish acknowledgements from relays must be forwarded back as normal `WorkerMessage::ConnectionStatus` payloads.
- FR-26: The client-side `connections` worker must forward all downstream `WorkerMessage` payloads to `parser` except `ConnectionStatus` messages with `status = "AUTH"`.
- FR-27: `ConnectionStatus(status = "AUTH")` must be routed through the existing crypto AUTH path.
- FR-28: Incoming proxy AUTH challenges must be translated by `connections` into the existing `SignerRequest(AuthEvent)` message for `crypto`.
- FR-29: The `SignerRequest(AuthEvent)` payload must remain JSON with `challenge`, `relay`, and `created_at` so the current signer implementation can be reused.
- FR-30: `crypto` AUTH signing results must continue returning through the existing `SignerResponse` path.
- FR-31: Client `connections` must translate the AUTH signing result into a JSON proxy command of the form `{ "type": "auth_response", "relay": "<url>", "event": <signed_event_json> }`.
- FR-32: The proxy may drop some subscriptions across disconnect, reconnect, backgrounding, or similar lifecycle churn in Phase 1.
- FR-33: Phase 1 does not require perfect subscription restoration after reconnect.

## 5. Non-Goals

- No redesign of parser, cache, or crypto worker responsibilities.
- No new JSON subscription protocol between client and proxy.
- No server-side parsed-event contract.
- No server-side cache semantics.
- No global cross-session subscription coalescing requirement.
- No global deduplication across all clients.
- No guarantee of perfect reconnect recovery or full subscription restoration in Phase 1.

## 6. Technical Considerations

- Existing command payloads already exist in [schemas/main.fbs](/root/code/nipworker/schemas/main.fbs):
  - `MainMessage`
  - `Subscribe`
  - `Unsubscribe`
  - `Publish`
- Existing downstream worker payloads already exist in [schemas/message.fbs](/root/code/nipworker/schemas/message.fbs):
  - `WorkerMessage`
  - `NostrEvent`
  - `ConnectionStatus`
  - `Raw`
- `Subscribe.subscription_id` already exists and should be treated as the subscription routing key.
- `Unsubscribe.subscription_id` already exists and is sufficient to clear subscription-scoped dedup state.
- `Publish.publish_id` exists in the current schema, but Phase 1 proxy routing should not depend on it.
- `WorkerMessage.sub_id` already exists and should be used for downstream routing.
- Main thread currently sends `MainMessage` to parser in [src/parser/src/lib.rs](/root/code/nipworker/src/parser/src/lib.rs), not directly to `connections`.
- Cache currently sends relay JSON `Envelope` payloads to `connections` in [src/cache/src/lib.rs](/root/code/nipworker/src/cache/src/lib.rs); proxy mode should preserve that cache contract.
- The current connection message builder in [src/connections/src/fb_utils.rs](/root/code/nipworker/src/connections/src/fb_utils.rs) already maps `AUTH` into `ConnectionStatus`; the proxy should follow the same Phase 1 representation unless the shared schema is expanded later.
- The current AUTH request path in [src/connections/src/connection.rs](/root/code/nipworker/src/connections/src/connection.rs) sends `SignerRequest(AuthEvent)` to `crypto`, with JSON payload `{ challenge, relay, created_at }`.
- The current AUTH response path returns `SignerResponse`, and `connections` uses the returned signed event plus relay URL to continue transport handling.
- The proxy should prefer one `WorkerMessage` FlatBuffer per downstream WebSocket frame in Phase 1 to keep implementation simple and debuggable.
- Publish acknowledgements should correlate by event ID from relay `OK` handling.
- Proxy mode should preserve the existing crypto-side AUTH request/response shapes and do any proxy-specific translation only inside `connections`.
- Phase 1 signed AUTH responses should be sent upstream to the proxy as a small JSON command rather than a new FlatBuffer schema, matching the existing use of JSON envelopes on the cache-to-connections side.
- If batching is added later, it should be treated as a separate protocol revision.

## 7. Success Metrics

- A proxy-mode client uses one browser WebSocket instead of one socket per relay.
- Applications can enable proxy-backed relay transport with `proxy?: { url: string }`.
- Existing client subscription APIs still function in proxy mode.
- Duplicate events from multiple relays are reduced before reaching the browser.
- Proxy-to-client transport is binary FlatBuffers end to end for normal operation.
- AUTH challenges can be forwarded through the client transport path without introducing a separate JSON control channel.

## 8. Client Architecture

### Current Worker Topology

The current client runtime in [src/index.ts](/root/code/nipworker/src/index.ts) creates four workers:

- `connections`
- `parser`
- `cache`
- `crypto`

`NostrManager` is mostly responsible for worker creation and `MessageChannel` wiring. That topology should remain the same in both local mode and proxy mode.

### Design Principle

Do not replace the four-worker client runtime.

Instead, replace the implementation behind the `connections` worker role:

- local mode: `connections` owns direct relay sockets
- proxy mode: `connections` owns one WebSocket to the relay proxy

This keeps parser, cache, and crypto wiring stable.

### Responsibilities by Mode

Local mode `connections`:

- receives outbound relay work from the local worker graph
- manages relay sockets
- reads raw relay frames
- converts relay traffic into `WorkerMessage`
- routes messages to parser or crypto

Proxy mode `connections`:

- opens one WebSocket to the TypeScript relay proxy
- forwards existing outbound client messages upstream
- receives binary `WorkerMessage` payloads downstream
- routes parseable network traffic to `parser`
- routes relay AUTH challenge traffic into the existing crypto-side path
- sends signed AUTH responses back upstream through the same proxy socket

The proxy-backed `connections` worker should not:

- own per-relay browser sockets
- implement relay fanout in the browser
- emit cache semantics such as `Eoce`

## 9. Proxy-Mode Data Flow

### Outbound

1. Main thread sends `MainMessage` bytes to `parser`.
2. `parser` and `cache` continue sending their existing outbound network work to `connections`.
3. `connections` translates those existing client-side messages into proxy WebSocket traffic.
4. The proxy uses the embedded subscription and relay information to manage upstream relay work.

Phase 1 outbound sources into `connections` are:

- parser-driven subscription lifecycle messages
- cache-driven relay `Envelope` payloads
- crypto-driven AUTH signing responses

The current client already sends `MainMessage` from main to parser, and cache already sends relay JSON `Envelope` payloads to `connections`. Proxy mode should preserve those internal contracts and do translation inside `connections`.

### Inbound

1. Proxy receives upstream relay frames.
2. Proxy converts those frames into `WorkerMessage` FlatBuffers.
3. Proxy sends binary `WorkerMessage` frames to the client over the single WebSocket.
4. Client `connections` receives each binary frame.
5. Client `connections` routes the frame to the correct local worker.

## 10. Routing Rules

### Parser-bound traffic

Forward directly to `parser`:

- `WorkerMessage::NostrEvent`
- `WorkerMessage::Raw`
- `WorkerMessage::ConnectionStatus` for normal relay status handling such as `NOTICE`, `OK`, and `CLOSED`

These should be passed through without rewriting if the payload is already valid.

### Crypto-bound traffic

AUTH challenge traffic should be intercepted by `connections` and forwarded into the existing crypto-side flow.

Phase 1 rule:

- if `WorkerMessage.type == ConnectionStatus`
- and `ConnectionStatus.status == "AUTH"`
- route the challenge through the existing client AUTH signing path instead of treating it as normal parser traffic

The payload needed for AUTH handling is:

- relay URL
- challenge string from `ConnectionStatus.message`
- `sub_id` if present

The existing crypto-side AUTH request shape should be preserved:

- `connections` builds `SignerRequest(AuthEvent)`
- payload is JSON containing `challenge`, `relay`, and `created_at`

### Upstream AUTH Response Flow

1. Proxy sends an AUTH challenge as `WorkerMessage::ConnectionStatus`.
2. `connections` detects `status == "AUTH"`.
3. `connections` translates the challenge into the existing `SignerRequest(AuthEvent)` message for `crypto`.
4. `crypto` returns the existing `SignerResponse` AUTH signing result.
5. `connections` sends that signed AUTH response back to the proxy over the single proxy WebSocket as a small JSON command:
   `{ "type": "auth_response", "relay": "<url>", "event": <signed_event_json> }`
6. Proxy forwards the signed AUTH response to the correct relay.

## 11. Expected Client Changes

### NostrManager

`NostrManager` in [src/index.ts](/root/code/nipworker/src/index.ts) should:

- accept a runtime `proxy` setting
- keep creating four workers in both modes
- pass proxy configuration to the `connections` worker during initialization

It should not:

- branch the whole worker graph for proxy mode
- bypass the `connections` worker and talk directly to proxy from main
- create a new direct main-to-connections control path

### Connections Worker Bootstrap

The worker bootstrap in [src/connections/index.ts](/root/code/nipworker/src/connections/index.ts) should accept proxy configuration at init time.

Recommended init shape:

```ts
type InitConnectionsMsg = {
	type: 'init';
	payload: {
		mainPort: MessagePort;
		cachePort: MessagePort;
		parserPort: MessagePort;
		cryptoPort: MessagePort;
		proxy?: {
			url: string;
		};
	};
};
```

If `proxy` is present, `connections` starts the proxy-backed implementation. If absent, it starts the existing local implementation.

## 12. Risks

- AUTH may not always carry a useful `sub_id`, so routing may need to rely on relay URL in some cases.
- The existing parser may currently receive some status traffic that proxy mode now wants to intercept in `connections`; that split must be explicit and tested.
- Reconnect semantics in Phase 1 are intentionally weak, so the worker should favor simplicity over perfect subscription restoration.

## 13. Open Questions

- None for Phase 1 protocol shape.
