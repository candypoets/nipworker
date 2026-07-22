//! Criterion micro-benchmarks for the hot paths called out in the perf work
//! (see commit 7ce168e "Performance pass: parser, connections, cache, proxy"):
//!
//! - `frame_scan`: zero-copy relay frame scanner vs a serde_json DOM parse.
//! - `kind1_parse`: kind-1 parsing with hoisted LazyLock regexes vs compiling
//!   the same 13 regexes per event (the pre-hoist behavior).
//! - `nostrdb_query`: top-k / borrow-don't-clone query path on a 10k-event DB.
//! - `batch_buffer`: parser→main batching fill/flush hot path.
//!
//! Run with:
//!   cargo bench --manifest-path crates/core/Cargo.toml \
//!       --features parser,cache,connections

#[cfg(all(feature = "parser", feature = "cache", feature = "connections"))]
mod perf_benches {
	use criterion::{black_box, criterion_group, BatchSize, BenchmarkId, Criterion, Throughput};
	use rand::rngs::SmallRng;
	use rand::{Rng, SeedableRng};

	use nipworker_core::generated::nostr::fb;
	use nipworker_core::parser::Parser;
	use nipworker_core::storage::db::index::NostrDB;
	use nipworker_core::storage::db::types::QueryFilter;
	use nipworker_core::transport::frame_scan::scan_relay_frame;
	use nipworker_core::types::nostr::{Event, EventId, PublicKey};
	use nipworker_core::worker::batch_buffer::BatchBufferManager;

	// ---------------------------------------------------------------------
	// Shared synthetic-data helpers
	// ---------------------------------------------------------------------

	fn hex_bytes(rng: &mut SmallRng, n: usize) -> String {
		const HEX: &[u8; 16] = b"0123456789abcdef";
		(0..n)
			.map(|_| HEX[rng.gen_range(0..16)] as char)
			.collect()
	}

	/// A kind-1 content body rich in mentions/hashtags/links/emojis.
	fn rich_content(rng: &mut SmallRng, target_len: usize) -> String {
		let mut content = String::with_capacity(target_len + 64);
		let snippets = [
			"gm #nostr, check this note",
			" nostr:npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqps4ka9",
			" note1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqykul8",
			" nevent1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
			" https://example.com/article/some-post",
			" https://cdn.example.com/img/photo.png",
			" https://video.example.com/clip.mp4",
			" #bitcoin #nostr #zapathon",
			" :wave: :fire:",
			" cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbXX1dfQ",
			" ```rust\\nfn main() { println!(\\\"hi\\\"); }\\n```",
		];
		while content.len() < target_len {
			content.push_str(snippets[rng.gen_range(0..snippets.len())]);
			content.push(' ');
		}
		content.truncate(target_len);
		// Keep the frame valid JSON: no raw control chars/quotes in content,
		// and never end on a lone backslash from a truncated `\n` escape.
		while content.ends_with('\\') {
			content.pop();
		}
		content
	}

	/// A realistic plain-text note: a couple of sentences, no hashtags, links,
	/// mentions, emojis, or other markup the content regexes look for.
	fn plain_content(rng: &mut SmallRng, target_len: usize) -> String {
		let mut content = String::with_capacity(target_len + 64);
		let snippets = [
			"gm everyone, had a great morning walk by the river today.",
			" The coffee shop on the corner finally reopened after renovations.",
			" I finished reading that book about distributed systems last night.",
			" Thinking about what to cook for dinner, maybe pasta again.",
			" The weather has been surprisingly warm for this time of year.",
		];
		while content.len() < target_len {
			content.push_str(snippets[rng.gen_range(0..snippets.len())]);
		}
		content.truncate(target_len);
		content
	}

	/// Build a realistic `["EVENT","sub_id",{...}]` relay frame of ~target_len.
	fn make_event_frame(rng: &mut SmallRng, target_len: usize) -> String {
		let id = hex_bytes(rng, 64);
		let pubkey = hex_bytes(rng, 64);
		let sig = hex_bytes(rng, 128);
		let mention = hex_bytes(rng, 64);
		let tag = hex_bytes(rng, 64);
		let created_at = 1_700_000_000 + rng.gen_range(0..2_592_000u64);
		let prefix = format!(
			"[\"EVENT\",\"sub_bench\",{{\"id\":\"{id}\",\"pubkey\":\"{pubkey}\",\"created_at\":{created_at},\"kind\":1,\"tags\":[[\"p\",\"{mention}\"],[\"e\",\"{tag}\",\"\",\"root\"],[\"t\",\"nostr\"]],\"content\":\""
		);
		let suffix = format!("\",\"sig\":\"{sig}\"}}]");
		let content_len = target_len.saturating_sub(prefix.len() + suffix.len());
		let content = rich_content(rng, content_len);
		format!("{prefix}{content}{suffix}")
	}

	fn make_kind1_event(rng: &mut SmallRng) -> Event {
		Event {
			id: EventId(rng.gen()),
			pubkey: PublicKey(rng.gen()),
			created_at: 1_700_000_000 + rng.gen_range(0..2_592_000u64),
			kind: 1,
			tags: vec![
				vec!["p".to_string(), hex_bytes(rng, 64)],
				vec![
					"e".to_string(),
					hex_bytes(rng, 64),
					"wss://relay.example.com".to_string(),
					"root".to_string(),
				],
				vec![
					"emoji".to_string(),
					"wave".to_string(),
					"https://cdn.example.com/emoji/wave.png".to_string(),
				],
				vec!["t".to_string(), "nostr".to_string()],
			],
			content: rich_content(rng, 1024),
			sig: hex_bytes(rng, 128),
		}
	}

	// ---------------------------------------------------------------------
	// frame_scan: zero-copy scanner vs serde_json DOM
	// ---------------------------------------------------------------------

	fn bench_frame_scan(c: &mut Criterion) {
		let mut group = c.benchmark_group("frame_scan");
		let mut rng = SmallRng::seed_from_u64(0x5eed_0001);

		for size in [1_024usize, 16 * 1024, 64 * 1024] {
			let frame = make_event_frame(&mut rng, size);
			group.throughput(Throughput::Bytes(frame.len() as u64));

			group.bench_with_input(
				BenchmarkId::new("scan_relay_frame", size),
				&frame,
				|b, frame| {
					b.iter(|| {
						let scanned = scan_relay_frame(black_box(frame)).expect("valid frame");
						black_box(scanned.kind);
						for arg in scanned.args.iter().flatten() {
							black_box(arg.raw);
						}
					})
				},
			);

			group.bench_with_input(
				BenchmarkId::new("serde_json_dom", size),
				&frame,
				|b, frame| {
					b.iter(|| {
						let value: serde_json::Value =
							serde_json::from_str(black_box(frame)).expect("valid json");
						black_box(&value);
					})
				},
			);
		}
		group.finish();
	}

	// ---------------------------------------------------------------------
	// kind1_parse: hoisted LazyLock regexes vs Regex::new per event
	// ---------------------------------------------------------------------

	/// The exact patterns the kind-1 path uses (parser::content + parser::kind1).
	/// Pre-hoist code ran `Regex::new` for each of these on every event.
	const KIND1_PATTERNS: &[&str] = &[
		r"```([\s\S]*?)```",
		r"(cashuA[A-Za-z0-9_-]+)",
		r"(^|[\s\x22\x27(\]])(#[a-zA-Z0-9_]+)",
		r"(?i)(https?://[^\s\\]+\.(jpg|jpeg|png|gif|webp|svg|ico)(\?[^\s\\]*)?)",
		r"(?i)(https?://[^\s\\]+\.(mp4|mov|avi|mkv|webm|m4v)(\?[^\s\\]*)?)",
		r"(?i)https?://[^\s\]\)\\]+",
		r"(?i)(nostr:([a-z0-9]+)|(nevent|nprofile|npub|naddr|note)1[a-z0-9]+)",
		r":([a-zA-Z0-9_-]+):",
		r"(?:nostr:)?(npub1[a-z0-9]+)",
		r"(?:nostr:)?(nprofile1[a-z0-9]+)",
		r"(?:nostr:)?(note1[a-z0-9]+)",
		r"(?:nostr:)?(nevent1[a-z0-9]+)",
		r"(?:nostr:)?(naddr1[a-z0-9]+)",
	];

	/// Pre-hoist baseline: compile every regex per event, then run the same
	/// scan work over the content.
	fn compile_and_scan_per_event(content: &str) -> usize {
		let mut matches = 0;
		for pattern in KIND1_PATTERNS {
			let re = regex::Regex::new(pattern).expect("valid pattern");
			matches += re.find_iter(content).count();
		}
		matches
	}

	fn bench_kind1_parse(c: &mut Criterion) {
		let mut group = c.benchmark_group("kind1_parse");
		let mut rng = SmallRng::seed_from_u64(0x5eed_0002);
		let parser = Parser::new(None);
		let event = make_kind1_event(&mut rng);

		group.bench_function("lazylock_statics", |b| {
			b.iter(|| {
				let (parsed, _requests) =
					parser.parse_kind_1(black_box(&event)).expect("kind 1 parses");
				black_box(parsed.parsed_content.len());
				black_box(parsed.profile_mentions.len());
				black_box(parsed.event_refs.len());
			})
		});

		group.bench_function("regex_new_per_event", |b| {
			b.iter(|| black_box(compile_and_scan_per_event(black_box(&event.content))))
		});

		// Plain-text note: no markup, so literal pre-checks skip every regex scan.
		let plain_event = Event {
			content: plain_content(&mut rng, 1024),
			..make_kind1_event(&mut rng)
		};

		group.bench_function("lazylock_statics_plain", |b| {
			b.iter(|| {
				let (parsed, _requests) =
					parser.parse_kind_1(black_box(&plain_event)).expect("kind 1 parses");
				black_box(parsed.parsed_content.len());
			})
		});

		group.finish();
	}

	// ---------------------------------------------------------------------
	// nostrdb_query: top-k / borrow-don't-clone query path on 10k events
	// ---------------------------------------------------------------------

	const DB_EVENTS: usize = 10_000;
	const DB_PUBKEYS: usize = 500;
	/// Far above the working set so no eviction interferes with queries.
	const DB_CAPACITY: usize = 64 * 1024 * 1024;

	fn build_parsed_worker_message(
		builder: &mut flatbuffers::FlatBufferBuilder,
		id: &str,
		pubkey: &str,
		kind: u16,
		created_at: u32,
	) -> Vec<u8> {
		builder.reset();
		let id_off = builder.create_string(id);
		let pubkey_off = builder.create_string(pubkey);
		let tags_off = builder.create_vector::<flatbuffers::WIPOffset<fb::StringVec>>(&[]);
		let parsed = fb::ParsedEvent::create(
			builder,
			&fb::ParsedEventArgs {
				id: Some(id_off),
				pubkey: Some(pubkey_off),
				kind,
				created_at,
				tags: Some(tags_off),
				..Default::default()
			},
		);
		let sub_id_off = builder.create_string("save_to_db");
		let message = fb::WorkerMessage::create(
			builder,
			&fb::WorkerMessageArgs {
				sub_id: Some(sub_id_off),
				content_type: fb::Message::ParsedEvent,
				content: Some(parsed.as_union_value()),
				..Default::default()
			},
		);
		builder.finish(message, None);
		builder.finished_data().to_vec()
	}

	fn pubkey_hex(index: usize) -> String {
		format!("{:064x}", index + 1)
	}

	/// Seed a DB with mixed kinds 0/1/3/7 from ~500 authors. Kind 1 dominates
	/// (the common timeline query target).
	fn seed_db() -> NostrDB {
		let db = NostrDB::new("bench-db".to_string(), DB_CAPACITY, vec![], vec![]);
		futures::executor::block_on(db.initialize()).expect("db init");

		let mut rng = SmallRng::seed_from_u64(0x5eed_0003);
		let mut builder = flatbuffers::FlatBufferBuilder::new();
		for i in 0..DB_EVENTS {
			// ~60% kind 1, ~25% kind 7, rest split between 0 and 3.
			let kind = match rng.gen_range(0..100u8) {
				0..=59 => 1,
				60..=84 => 7,
				85..=92 => 0,
				_ => 3,
			};
			let pubkey = pubkey_hex(rng.gen_range(0..DB_PUBKEYS));
			let created_at = 1_700_000_000 + rng.gen_range(0..2_592_000u32);
			let id = format!("{:064x}", i + 1);
			let bytes = build_parsed_worker_message(&mut builder, &id, &pubkey, kind, created_at);
			futures::executor::block_on(db.add_worker_message_bytes(&bytes)).expect("ingest");
		}
		db
	}

	thread_local! {
		static SEEDED_DB: std::cell::RefCell<Option<NostrDB>> = const { std::cell::RefCell::new(None) };
	}

	fn with_db<R>(f: impl FnOnce(&NostrDB) -> R) -> R {
		SEEDED_DB.with(|cell| {
			let mut borrow = cell.borrow_mut();
			if borrow.is_none() {
				*borrow = Some(seed_db());
			}
			f(borrow.as_ref().expect("seeded db"))
		})
	}

	fn kinds_limit_filter(kinds: &[u16], limit: usize) -> QueryFilter {
		let mut filter = QueryFilter::new();
		filter.kinds = Some(kinds.to_vec());
		filter.limit = Some(limit);
		filter
	}

	fn bench_nostrdb_query(c: &mut Criterion) {
		let mut group = c.benchmark_group("nostrdb_query");
		group.sample_size(30);

		for limit in [20usize, 100, 1000] {
			group.bench_function(BenchmarkId::new("kind1_limit", limit), |b| {
				with_db(|db| {
					b.iter(|| {
						let result = db
							.query_events_with_filter(black_box(kinds_limit_filter(&[1], limit)))
							.expect("query");
						black_box(result.events.len());
					})
				});
			});
		}

		group.bench_function("kind1_since_until_window", |b| {
			with_db(|db| {
				b.iter(|| {
					// One-hour window in the middle of the seeded range.
					let mut filter = kinds_limit_filter(&[1], 100);
					filter.since = Some(1_700_000_000 + 15 * 86_400);
					filter.until = Some(1_700_000_000 + 15 * 86_400 + 3_600);
					let result = db
						.query_events_with_filter(black_box(filter))
						.expect("query");
					black_box(result.events.len());
				})
			});
		});

		group.bench_function("kind1_single_author", |b| {
			with_db(|db| {
				b.iter(|| {
					let mut filter = kinds_limit_filter(&[1], 100);
					filter.authors = Some(vec![pubkey_hex(7)]);
					let result = db
						.query_events_with_filter(black_box(filter))
						.expect("query");
					black_box(result.events.len());
				})
			});
		});

		group.bench_function("kind1_ten_authors", |b| {
			with_db(|db| {
				b.iter(|| {
					let mut filter = kinds_limit_filter(&[1], 100);
					filter.authors = Some((0..10).map(pubkey_hex).collect());
					let result = db
						.query_events_with_filter(black_box(filter))
						.expect("query");
					black_box(result.events.len());
				})
			});
		});

		group.finish();
	}

	// ---------------------------------------------------------------------
	// batch_buffer: fill/flush hot path (16KB size threshold)
	// ---------------------------------------------------------------------

	fn bench_batch_buffer(c: &mut Criterion) {
		let mut group = c.benchmark_group("batch_buffer");

		for size in [500usize, 2_048, 8_192] {
			let data = vec![0xABu8; size];

			// Steady-state per-message cost, including threshold flushes.
			group.bench_with_input(BenchmarkId::new("add_message", size), &data, |b, data| {
				let mut mgr = BatchBufferManager::new();
				b.iter(|| {
					let flushed = mgr.add_message(black_box("sub_bench"), black_box(data));
					black_box(flushed.as_deref().map(<[u8]>::len));
				})
			});

			// Cost of filling a fresh buffer until the 16KB threshold flushes;
			// the return value is the frames-per-flush count for this size.
			group.bench_with_input(
				BenchmarkId::new("fill_until_flush", size),
				&data,
				|b, data| {
					b.iter_batched(
						BatchBufferManager::new,
						|mut mgr| {
							let mut frames = 0usize;
							loop {
								frames += 1;
								if mgr.add_message("sub_bench", data).is_some() {
									break;
								}
							}
							black_box(frames)
						},
						BatchSize::SmallInput,
					)
				},
			);
		}
		group.finish();
	}

	// ---------------------------------------------------------------------
	// sub_dedup: EVENT frame id extraction + per-sub mark (connections-layer
	// cross-relay dedup hot path)
	// ---------------------------------------------------------------------

	fn bench_sub_dedup(c: &mut Criterion) {
		use nipworker_core::transport::sub_dedup::{event_frame_id, SubDedup};

		let mut rng = SmallRng::seed_from_u64(42);
		let frame = format!(
			r#"["EVENT","sub_bench",{{"id":"{}","pubkey":"{}","kind":1,"content":"hello world","tags":[],"created_at":1700000000,"sig":"{}"}}]"#,
			hex_bytes(&mut rng, 64),
			hex_bytes(&mut rng, 64),
			hex_bytes(&mut rng, 128)
		);

		let mut group = c.benchmark_group("sub_dedup");

		// Zero-copy scan + hex decode of the event id out of a full EVENT frame.
		group.bench_function("extract_id", |b| {
			b.iter(|| black_box(event_frame_id(black_box(&frame))))
		});

		// Steady-state insert/probe cost of the per-sub id ring.
		group.bench_function("mark", |b| {
			let mut dedup = SubDedup::new();
			let mut counter = 0u64;
			b.iter(|| {
				let mut id = [0u8; 32];
				id[0..8].copy_from_slice(&counter.to_le_bytes());
				counter = counter.wrapping_add(1);
				black_box(dedup.mark(black_box(id)))
			})
		});

		group.finish();
	}

	criterion_group! {
		name = benches;
		config = Criterion::default();
		targets = bench_frame_scan, bench_kind1_parse, bench_nostrdb_query, bench_batch_buffer, bench_sub_dedup
	}
}

#[cfg(all(feature = "parser", feature = "cache", feature = "connections"))]
criterion::criterion_main!(perf_benches::benches);

#[cfg(not(all(feature = "parser", feature = "cache", feature = "connections")))]
fn main() {
	eprintln!("perf benches require: --features parser,cache,connections");
}
