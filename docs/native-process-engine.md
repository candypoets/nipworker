# Native process engine ownership

Apple and Android mobile wrappers acquire the Rust engine through
`nipworker_shared_acquire` and release their callback registration through
`nipworker_shared_release`. The first client creates the engine; later native
or React Native clients receive the same engine handle and therefore share the
same worker set, subscription buffers, cache, signer, and mesh state.

The first acquire owns the storage path, relay, and mesh configuration for that
engine lifetime. Later configuration calls do not create or reconfigure a
second engine. Mobile facades then idempotently retain a process-lifetime
anchor with `nipworker_shared_process_acquire`. Releasing the final runtime
client removes its callback and waits for callback snapshots containing its
userdata, but the anchor keeps the engine, workers, subscriptions, signer, and
in-memory cache alive for the next RN bridge/runtime generation.

The anchor is installed lazily with the first native manager initialization,
so host applications do not need custom `Application` or `AppDelegate`
integration. Its ownership is nevertheless process-scoped in Rust rather than
being tied to the Kotlin object, Swift facade, RN module, bridge, or Hermes
runtime that requested it.

Normal `RuntimeTransport` or native-manager invalidation must release only its
delivery client. `nipworker_shared_process_release` is reserved for explicit
application shutdown and deterministic test reset. It removes the anchor; the
engine is synchronously joined immediately if no delivery clients remain, or
after the last remaining client releases. Android exposes this operation as
`NipworkerRuntime.shutdownProcess()`, while Swift exposes
`NostrManager.shutdownProcessEngine()`. Process termination itself needs no
explicit call because the operating system reclaims the native image.

Callback fanout preserves the original owned packet for the first client, so
the normal one-client React Native path remains zero-copy. If pure-native Swift
and React Native actively consume the same engine simultaneously, additional
clients receive owned packet copies because the C callback contract transfers
packet ownership. Subscription payloads remain in the single Rust-owned fixed
capacity buffers and continue to be exposed to JSI as zero-copy ArrayBuffers.

React Native's `RuntimeTransport` remains runtime-scoped by design. It owns the
CallInvoker, wake coalescing, bounded control queue, and JSI buffer pins for one
Hermes generation; it is not a process singleton and is invalidated on reload.
That runtime state sits above the process-wide Rust engine and subscription
store.

The registry is process-wide only when all consumers link the same Rust binary
image. The supplied Apple XCFramework is static and must be linked once into
the final app; consumers must not embed separate dynamic frameworks containing
private copies. Android consumers must use the single
`libnipworker_native_ffi.so` supplied by the native AAR. A separate loaded copy
of the Rust library necessarily has separate static registry state.

At `info` log level, shared-registry lifecycle records include `generation`,
`handle`, `client`, `clients`, and `totalReferences`. A normal runtime reload is
visible as `release action=last-client-anchored`, followed by
`acquire action=reuse` with the same generation and handle. Explicit shutdown
is visible as `anchor action=final-release`, followed by
`lifecycle action=deinitialized`. Without an anchor, a legacy client sequence
may show `release action=final` before the next generation's
`acquire action=create-start`/`created`; the lifecycle mutex still guarantees
those generations cannot overlap.
