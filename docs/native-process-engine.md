# Native process engine ownership

Apple and Android mobile wrappers acquire the Rust engine through
`nipworker_shared_acquire` and release their callback registration through
`nipworker_shared_release`. The first client creates the engine; later native
or React Native clients receive the same engine handle and therefore share the
same worker set, subscription buffers, cache, signer, and mesh state.

The first acquire owns the storage path, relay, and mesh configuration for that
engine lifetime. Later configuration calls do not create or reconfigure a
second engine. The final release synchronously joins the engine. Releasing one
client first removes its callback and waits for callback snapshots containing
its userdata before the wrapper may reclaim that context.

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
