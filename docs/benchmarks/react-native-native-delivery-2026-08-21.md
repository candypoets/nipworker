# React Native native-delivery benchmark — 2026-08-21

This host benchmark exercises the runtime-independent shared C++ delivery state. It sends
1,000,000 route callbacks from 16 native threads, drains the coalesced route set, then queues
and drains 100,000 owned 64-byte control packets.

Run it with:

```sh
npm run bench:native-transport
```

The first validation run measured:

| Metric                           |                Result |
| -------------------------------- | --------------------: |
| Route callbacks                  |             1,000,000 |
| Producer threads                 |                    16 |
| Unique dirty routes drained      |                   256 |
| Route scheduler wakes            |                     1 |
| Route enqueue throughput         | 8,561,490 callbacks/s |
| Route drain time                 |              0.018 ms |
| Control packets accepted/drained |     100,000 / 100,000 |
| Control scheduler wakes          |                     1 |
| Control packets dropped          |                     0 |
| Control enqueue throughput       |  17,389,200 packets/s |
| Control queue byte high-water    |       6,400,000 bytes |
| Process maximum RSS              |            15,808 KiB |

The legacy comparison is architecture-derived from the retained Android incident, not a
timing rerun of the crashing binary. For each native callback, the old hot path allocated and
copied a JNI byte array, copied it again into the C++ vector queue, and issued one generated
event-emitter wake. For 1,000,000 modeled route callbacks that is 1,000,000 wakes,
1,000,000 JNI byte-array allocations, and 2,000,000 payload copies. The shared transport
measured one route wake, a 99.9999% reduction, while subscription payloads remain in their
Rust-owned pinned buffers with zero transport copies.

The release parity guard also forbids retaining the old delivery path as a fallback. Android
must not expose `onNativeData`, `nativeQueueData`, process-global drain queues, or generated
`emitOnData`; iOS must not use main-queue/generated-emitter wakes or process-global runtime
installation state; TypeScript and the TurboModule spec must not retain `NativeEventEmitter`
or `onData` compatibility delivery. Both platform adapters must reference the shared transport
and schedule through `CallInvoker::invokeAsync`.

Host timings vary with CPU load. The durable regression criteria are one scheduled route wake
per undrained burst, complete route/control drain, zero unexpected drops, exact drop counters
under configured saturation, and queue high-water never exceeding its configured byte bound.
On control saturation the explicit policy rejects and immediately frees the newest packet,
preserving the accepted FIFO and causal ordering of signer/auth responses already queued.

## Native engine lifecycle probe

Run `npm run bench:native-lifecycle` on Linux to build the host native FFI and measure process
threads and resident memory across 25 real `nipworker_init`/`nipworker_deinit` cycles. Before
the shutdown fix, a fresh process grew from 1 to 126 threads (exactly 5 retained threads per
cycle) and from 12,276 KiB to 18,472 KiB RSS (+6,196 KiB) without subscriptions or relay
traffic. The same probe must show no retained native worker threads after teardown before this
transport is released; RSS can vary with allocator caching, so thread return and a stable RSS
plateau across repeated runs are the durable gates.

After explicit engine and worker shutdown was added, the probe retained zero threads: the Node
host stayed at 11 threads after both 25 and 100 cycles. RSS rose once from 61,628 KiB to
74,848 KiB after 25 cycles (+13,220 KiB), then remained at the same plateau in the longer run
(61,532 KiB to 74,732 KiB after 100 cycles, +13,200 KiB). This replaces the linear five-thread
growth with synchronous teardown; the one-time RSS increase is allocator/runtime warm-up rather
than per-cycle growth.
