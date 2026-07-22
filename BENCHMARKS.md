# Benchmarks

This repo's benchmark harness has two layers:

| Layer | Command | What it measures |
|---|---|---|
| Rust micro-benchmarks (criterion, native) | `npm run bench` | Hot-path internals: relay frame scanner, kind-1 parsing, NostrDB queries, batch buffer |
| Browser end-to-end (Playwright + real WASM workers) | `npm run bench:browser` | Real product: parser→main throughput, cache query latency in WASM, end-to-end event latency |

Neither runs as part of `npm test` / `npm run test:e2e` — they are measurement tools, not pass/fail gates.

## Baseline results

Environment: AMD/Linux, rustc release profile (`opt-level=3, lto=fat, codegen-units=1`), Chromium via Playwright. Absolute numbers vary by machine; use them for relative comparisons and regression checks (`cargo bench -- --baseline <name>`).

### Rust micro-benchmarks (`crates/core/benches/perf.rs`)

**frame_scan** — zero-copy scanner vs `serde_json::Value` DOM parse on the same EVENT frames:

| Frame size | `scan_relay_frame` | `serde_json` DOM | Ratio |
|---|---|---|---|
| 1 KB | 632 ns (1.51 GiB/s) | 1.29 µs | **2.0× faster** |
| 16 KB | 10.5 µs (1.45 GiB/s) | 4.77 µs | ~2.2× slower |
| 64 KB | 36.3 µs (1.68 GiB/s) | 16.4 µs | ~2.2× slower |

Nuanced finding: the scanner wins at typical small frames but serde_json's memchr-based string scanning is faster per byte on large frames. The scanner's real value is avoiding DOM allocation and reserialization downstream (the old path parsed **twice** and rebuilt the frame), not raw scan speed. The 16 KB point showed high variance (±15%).

**kind1_parse** — real `Parser::parse_kind_1` (regexes hoisted to `LazyLock` statics) vs recompiling the same 13 patterns per event:

| Path | Mean | Ratio |
|---|---|---|
| LazyLock statics (current) | 92.6 µs | — |
| `Regex::new` per event (pre-`7ce168e` behavior) | 927 µs | **~10× slower** |

Strongly validates the regex-hoisting work. Note: the current kind-1 path uses 13 hoisted patterns, so the honest comparison is 13 compilations/event — the "~23" figure in commit `7ce168e` was an overestimate.

**nostrdb_query** — 10k-event `NostrDB` (60% kind 1, 25% kind 7, 500 pubkeys, 30-day `created_at` spread):

| Query | Mean |
|---|---|
| kind-1, limit 20 | 99.1 µs |
| kind-1, limit 100 | 114 µs |
| kind-1, limit 1000 | 166 µs |
| kind-1, 1h since/until window, limit 100 | 20.2 µs |
| kind-1, single author | 1.39 µs |
| kind-1, ten authors | 13.7 µs |

Latency scales with `limit`, not candidate count — consistent with the top-k / index-driven query design (`1758ca2`, `274c188`).

**batch_buffer** — `BatchBufferManager` (16 KB threshold):

| Frame size | `add_message` | Frames per 16 KB flush |
|---|---|---|
| 500 B | 155 ns | ~32 |
| 2 KB | 255 ns | 8 |
| 8 KB | 507 ns | 2 |

### Browser end-to-end (`tests/bench/`, mock relay, no network)

**Throughput** (parser → main, kinds:[1] events):

| Events | Wall time | Events/sec | Batches | Avg batch size |
|---|---|---|---|---|
| 100 | 606 ms* | 165 | 5 | 20 |
| 1,000 | 121 ms | 8,369 | 38 | 27 |
| 10,000 | 455 ms | **22,019** | 195 | 51 |

\* n=100 includes relay connect + worker warm-up (~600 ms first event); warm runs reach first event in ~7–11 ms.

**Cache query latency in WASM** (cacheOnly, 20 repeats each):

| Limit | Mean | p50 | p95 | p99 |
|---|---|---|---|---|
| 20 | 0.96 ms | 0.7 | 1.5 | 2.2 |
| 100 | 1.32 ms | 1.3 | 1.5 | 1.8 |
| 1000 | 7.41 ms | 6.9 | 9.4 | 11.4 |

**End-to-end latency**: REQ→first event 1.8 ms; REQ→last cached event 12.6 ms; live-event one-way (relay timestamp → callback) avg 8.4 ms / p50 2 ms / p95 68 ms over a 200-event burst (~2,762 live events/sec).

## Notable findings from bring-up

- **Ring-buffer sizing is the memory bound, by design**: each subscription's ring buffer is fixed at `limit × bytesPerEvent` — this is the deliberate mechanism that keeps per-subscription memory bounded regardless of relay behavior. The bench reproduced the consequence: a 200-event live burst into a sub configured `limit: 1` dropped 189 events with "buffer full" warnings. That's intended backpressure, not a bug — but it means `limit` is a memory/burst-tolerance knob, not just a result-count knob, and one-time-query-style configs (tiny limit, live sub still open) will drop under bursts. Size for expected burst, or use `closeOnEose` for one-shot queries.
- **Batching works as designed**: average batch size grows with load (20 → 51 events/message), confirming the 16 KB threshold dominates the 50 ms timer under burst.
- **Kind-6/7 events without reference tags are dropped** by the parser pipeline (~15% of a naive synthetic mix). Expected behavior, but it skews naive throughput counts — bench filters use `kinds: [1]` for exactness.
- The PRD claim "MessageChannel throughput: 50K–200K msg/sec" is consistent with what we see: 22k events/sec were delivered in only ~195 postMessages/sec thanks to batching — two orders of magnitude of headroom.

## Follow-up optimizations (post-baseline)

**frame_scan memchr rewrite** — the inner loops of `frame_scan.rs` now skip via `memchr::memchr2/3` instead of byte-at-a-time. The scanner went from ~2.2× slower than serde_json on large frames to faster everywhere:

| Frame size | Before | After | vs serde_json |
|---|---|---|---|
| 1 KB | 632 ns | 500 ns (1.9 GiB/s) | 2.4× faster |
| 16 KB | 10.5 µs | 1.25 µs (12.2 GiB/s) | 3.8× faster |
| 64 KB | 36.3 µs | 4.29 µs (14.2 GiB/s) | 4.1× faster |

**Content-parser regex pre-checks** — all 13 regex scans in `content.rs`/`kind1.rs` are now guarded by mandatory-literal substring checks (`might_match` / `prescan_content`). Result: **~10% on markup-free notes only** (8.9µs → 8.1µs on the plain fixture), nothing on rich content. The `regex` crate already does SIMD literal prefiltering internally, so most of the anticipated win didn't exist — and a naive byte-loop pre-check actually *regressed* plain text 85% before the memchr2 rewrite. Kept because it's never slower and the guards document each pattern's required literals. A `lazylock_statics_plain` bench was added to `perf.rs` to track this.

Lesson recorded: measure before assuming — the 92.6µs `parse_kind_1` cost is mostly *matching*, not scanning-no-match, so further kind-1 wins would come from reducing per-match allocation, not more prefiltering.

**Batch timeout retune** (`BATCH_TIMEOUT_MS` 50→8, sweeper 25→4ms; `batch_buffer.rs:23`, `parser_worker.rs:30`) — the 50ms timer was the entire live-event tail latency. Measured via the browser bench (200-event live burst):

| Metric | 50/25ms | 8/4ms (kept) |
|---|---|---|
| live p95 | 58–68 ms | **11–15 ms** |
| live avg | 8.3 ms | 2.9–4.4 ms |
| live events/sec | 3,247 | ~12,000–13,700 |
| 10k throughput | 18,618 ev/s | **~23,000–24,000 ev/s** (+~29%) |

Batch count *fell* (299→102 per 10k) — the 16KB size threshold dominates bursts; the timer now only handles trickle traffic. Memory implication: same total payload bytes and the same preallocated 16KB per-sub buffers; only slightly more postMessage framing overhead at low event rates.

**wasm-opt -O3 A/B — negative result, `--no-opt` stays.** Binaryen 131 `-O3` shrank all four WASM binaries 14–19% but *regressed* every throughput row 12–22% across two runs (10k: 24,049 → ~18,900–21,000 ev/s). Binaries restored; commit `8b05cf0`'s decision to drop wasm-opt in favor of rustc-side opts is now validated by measurement instead of folklore.

**Build-config matrix** (all 4 crates rebuilt per config, ≥2 bench runs each, plus a 3-round interleaved A/B for the head-to-head; rustc 1.93 — bulk-memory/mutable-globals/sign-ext/reference-types/multivalue are already default-on):

| Config | 10k ev/s | cache p50 L=1000 | parser .wasm | Verdict |
|---|---|---|---|---|
| baseline (opt3, lto=fat) | ~23.6k | 6.6–7.4 ms | 2,912,122 | reference |
| +simd128 | ~24.9k (**+3.5% interleaved**, won 3/3 pairs) | neutral | 2,846,536 (−2.3%) | real but sub-threshold; **not adopted** |
| simd + opt-level=s | ~22.3k | neutral | 2,441,029 (−16%) | size/speed trade, not adopted |
| simd + opt-level=z | ~18k ❌ | +25–50% ❌ | 2,008,883 (−31%) | clear loser |

simd128 is consistent (won every interleaved pair, no regression anywhere) but below the 10% adoption bar, and it hard-requires Chrome 91+/FF 89+/Safari 16.4+ — older engines fail to compile the module outright. To adopt later: `crates/<x>/.cargo/config.toml` with `[target.wasm32-unknown-unknown] rustflags = ["-C", "target-feature=+simd128"]`.

**Early dedup + drop audit (post-multirelay round).** Full audit of the incoming path: **no event drops** — the parser shard dispatchers' `try_send` falls back to blocking `send` (backpressure, `parser_worker.rs:211-228`), all cross-worker channels are unbounded, and the 10k `seen_ids` cap degrades dedup rather than dropping (stops inserting, no log). The one genuinely silent drop found was `close_sub()` losing CLOSE frames on a full send queue (`transport/connection.rs:652`) — now warn-logged. Outgoing-frame drops (send-queue full, cooldown, retries exhausted) were already logged. Empirically: exact unique counts in every bench run.

Cross-relay dedup moved into the connections worker (`transport/sub_dedup.rs`: per-subId bounded ring, 4096 ids FIFO, freed on CLOSE; zero-copy id extraction ~0.5µs/frame). ~38k duplicate frames at ×25 no longer reach the parser; dups still exactly 0; parser-side dedup kept as safety net. **Throughput effect: small (~+9% at ×10, flat elsewhere)** — because the parser already deduped *before* parsing, duplicates never cost a full parse; the savings are only the channel hop + FlatBuffer wrap per dup. Conclusion: the multi-relay gap vs single-relay sits earlier in the pipe (per-frame WebSocket/gloo receipt + the connections worker's own per-frame scan/build), not in dedup. That is the next measurable target.

**Parser allocation audit** (`content.rs`, `kind1.rs`) — removed per-event deep clones of ContentBlocks (identity rebuild + double `clone()` for shorten/assign), a redundant second regex pass per match (`find_iter().collect()` + `captures()` → single `captures_iter`), unguarded 3× `str::replace` on every text block, `to_lowercase()` allocations in `process_nostr`/`process_link`/`is_hex64`, clone-heavy `group_media`, and a dead `get_link_preview()` allocation. `shorten_content` now borrows (`&[ContentBlock]`) with a no-alloc fast path when nothing needs shortening. Result: **kind1_parse 87.8µs → ~61–63µs (−29%)** on rich content, −19% on plain. Deliberately not touched (documented in code review): `ContentBlock.block_type: String` → `&'static str` and owned-text blocks (pub API break), ring-buffer owned-byte returns (storage API boundary), per-event pubkey hex keys in mute/chat_limiter pipes (key-type redesign — flagged as follow-up).

## Head-to-head: nipworker vs NDK vs Welshman vs Nostrify vs nostr-tools

`npm run bench:compare` — identical scenario for every contender: same deterministic mock-relay stream (kinds:[1], single localhost relay, seeded), fresh page per contender, two full runs (stable within noise). Code: `tests/compare/` (own `package.json`, contenders are NOT root deps), `playwright.compare.config.ts`.

| Contender | n=1,000 ev/s | n=10,000 ev/s | First event | Long tasks | Jank frames (10k) | Heap Δ (10k) |
|---|---|---|---|---|---|---|
| **nipworker** | 6,227–6,566 | **21,745–22,110** | 8 ms | 0 | 0 | +97.7 MB* |
| nostr-tools (raw) | 22,624–23,310 | 24,050–24,319 | 0.7 ms | 0 | 0 | +0.9 MB |
| NDK | 13,333–15,060 | 3,063–3,308 ⚠️ | 11 ms | 0 | 27–31 | +2.5 MB |
| Welshman | 196 ⚠️ | 199 ⚠️ | 201 ms | 0 | 0 | ~0 |
| Nostrify | 8,278–8,503 | 9,764–10,433 | 1 ms | 0 | 0 | ~0 (43–49 MB peak transient) |

⚠️ NDK degrades superlinearly with n (subscription bookkeeping; reproducible). Welshman is throttled by design — its Socket `TaskQueue` delivers at batchSize=20/batchDelay=100ms (~200 msg/s), a queue-policy artifact, not parse cost.

**Fairness — per-event work by default (read before quoting):** signature verification is OFF in every row (mock relay serves unsigned events; every lib that verifies by default — nostr-tools, NDK, Nostrify, welshman's high-level API — was run with verify disabled). nipworker never verifies on ingest regardless. What each does per event: **nipworker** — full JSON→typed ParsedEvent parse, kind-specific content parsing, dedup (10k ring), FlatBuffer serialization, **and persists to IndexedDB**. nostr-tools — `JSON.parse` + filter match. NDK — JSON.parse + NDKEvent wrapping. Nostrify — JSON.parse + always-on zod message validation. Welshman — JSON.parse only at Socket level.

**Reading:** nipworker delivers raw-nostr-tools-class throughput (~22–24k ev/s) while doing strictly more work per event (full parse + persistence, off-main-thread), and beats every full client stack by 2–100× at n=10,000 with zero main-thread long tasks and zero jank.

\* **Memory is nipworker's weak spot in this table**: the +97.7 MB main-thread heap delta at n=10,000 is real but by-design — subscription ring buffers are sized `limit × bytesPerEvent` (bounded memory per sub), and CDP shows the heap returns to ~4 MB after teardown (no leak). `performance.memory` also sees only the main thread: nipworker's 4 worker heaps + WASM linear memory (~4 MB bytecode) are invisible to it, so its true total is higher than shown while the raw libs' totals are fully shown — the comparison is conservative for nipworker on speed and flattering on memory.

### Multi-relay (the realistic scenario)

`tests/compare/multirelay*` — same contenders × {5, 10, 25} relays, each relay serving the same 2,000-event set with **80% cross-relay overlap + 20% per-relay unique** (expected unique: 3,600 / 5,600 / 11,600). 15/15 green across 3 consecutive runs; unique/dup counts deterministic.

| Contender × relays | unique ev/s | **dups leaked to app** | connect-all | long tasks | heap Δ |
|---|---|---|---|---|---|
| nipworker ×5 / ×10 / ×25 | 3,880 / 4,075 / 2,320 | **0 / 0 / 0** | **11 / 11 / 15 ms** | 0 / 0 / 0 | ~20 MB* (flat in relays) |
| nostr-tools ×5 / ×10 / ×25 | 5,210 / 5,000 / 2,740 | 0 / 0 / 0 | 26 / 88 / **3,920 ms** | 0 / 0 / ~1 | 0.4–1.1 MB |
| NDK ×5 / ×10 / ×25 | 2,870 / 2,320 / **825** | 0 / 0 / 0 | 17 / 186 / **6,290 ms** | 0 | 1.3–3.7 MB |
| Welshman ×5 / ×10 / ×25 | 340 / 520 / 795 | **6,400 / 14,400 / 38,400** ⚠️ | 5 / 24 / 4,070 ms | 0 | ≤1 MB |
| Nostrify ×5 / ×10 / ×25 | 2,895 / 2,665 / 1,980 | **~1,600 / ~2,600 / ~16,200** ⚠️ | 85 / 168 / 4,890 ms | 0 | ≤1.5 MB |

Measured default dedup behavior: nipworker — full dedup in the parser worker *before* anything reaches main. nostr-tools — pool-level seen-id set, 0 dups. NDK — full dedup (38,400 suppressed at ×25) but superlinear bookkeeping cost. Welshman — **no dedup at Socket level**, app receives every duplicate. Nostrify — **partial**: its `CircularSet(1000)` evicts ids while a single relay streams 2,000, so duplicates leak past the window (leak count is interleaving-dependent; the leak itself is deterministic).

**Reading:** multi-relay is where the architecture argument lands. JS libs' connect storms happen on the main thread — 4–6 *seconds* to open 25 sockets vs nipworker's **15 ms** (worker-side connects). nostr-tools still edges raw unique-throughput (~5.2k vs ~3.9k at ×5) while doing far less per event, but the app-visible difference is what matters: nipworker delivers fully-deduped, fully-parsed, persisted events with a flat main thread at every scale, while Welshman floods the app with 38k duplicate callbacks and Nostrify silently leaks dups past its ring window. Caveats: nostr-tools needed its default ~4.4s connect timeout raised to survive the 25-socket browser connect storm (relays accept fine — it's browser-side); EOSE is aggregate-only for nostr-tools/NDK/Nostrify, per-relay for nipworker and Welshman; all relays are localhost processes, so this measures client pipeline cost, not network variance. \* heap is main-thread-only; worker/WASM heaps invisible (see single-relay note).

## Reproducing & comparing

```bash
npm run bench                                    # criterion, ~4 min
cargo bench --manifest-path crates/core/Cargo.toml \
  --features parser,cache,connections -- --baseline main   # save/compare baselines
npm run bench:browser                            # playwright, ~20 s, self-starts mock relay + vite on ports 7710/5375
npm run bench:compare                            # head-to-head single-relay + multi-relay (5/10/25) vs NDK/Welshman/Nostrify/nostr-tools
```

Files: `crates/core/benches/perf.rs`, `tests/bench/{mock-relay.mjs,bench.html,bench.ts,bench.spec.ts,vite.bench.config.mjs}`, `playwright.bench.config.ts`, `tests/compare/` + `playwright.compare.config.ts` (head-to-head).

Known pre-existing issue (unrelated to this harness): `tests/e2e-browser/` specs construct workers as `new Worker(new URL('./parser/index.js', ...))` which 404s under a plain vite dev server (on-disk files are `.ts`); the bench suite works around it with a dev middleware in `tests/bench/vite.bench.config.mjs`. The e2e specs were left untouched.
