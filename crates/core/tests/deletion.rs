//! NIP-09 deletion tombstones: public-surface integration test.
//!
//! Exercises the Storage trait path end to end: events persisted through the
//! public API must be filtered from query results once a valid kind 5
//! references them, and invalid deletions (author mismatch) must be ignored.

use nipworker_core::generated::nostr::fb;
use nipworker_core::storage::NostrDbStorage;
use nipworker_core::traits::Storage;
use nipworker_core::types::nostr::Filter;

fn build_parsed_worker_message(
    id: &str,
    pubkey: &str,
    kind: u16,
    created_at: u32,
    tags: &[&[&str]],
) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let id_off = builder.create_string(id);
    let pubkey_off = builder.create_string(pubkey);
    let tag_offsets: Vec<_> = tags
        .iter()
        .map(|tag| {
            let items: Vec<_> = tag.iter().map(|s| builder.create_string(s)).collect();
            let items = builder.create_vector(&items);
            fb::StringVec::create(&mut builder, &fb::StringVecArgs { items: Some(items) })
        })
        .collect();
    let tags_vec = builder.create_vector(&tag_offsets);
    let parsed = fb::ParsedEvent::create(
        &mut builder,
        &fb::ParsedEventArgs {
            id: Some(id_off),
            pubkey: Some(pubkey_off),
            kind,
            created_at,
            tags: Some(tags_vec),
            ..Default::default()
        },
    );
    let sub_id_off = builder.create_string("save_to_db");
    let message = fb::WorkerMessage::create(
        &mut builder,
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

fn hex_id(n: usize) -> String {
    format!("{:064x}", n)
}

fn query_kind(storage: &NostrDbStorage, kind: u16) -> Vec<Vec<u8>> {
    let mut filter = Filter::new();
    filter.kinds = Some(vec![kind]);
    futures::executor::block_on(storage.query(vec![filter])).unwrap()
}

#[tokio::test]
async fn deletion_filters_events_through_public_storage_api() {
    let storage = NostrDbStorage::new("deletion-it".to_string(), 1024 * 1024, vec![], vec![]);
    storage.initialize().await.unwrap();

    let author = hex_id(99);
    let other = hex_id(100);
    let address = format!("30009:{}:membership", author);

    let badge =
        build_parsed_worker_message(&hex_id(1), &author, 30009, 1000, &[&["d", "membership"]]);
    storage.persist(&badge).await.unwrap();
    assert_eq!(query_kind(&storage, 30009).len(), 1);

    // A deletion signed by another pubkey must not touch the badge.
    let forged = build_parsed_worker_message(&hex_id(2), &other, 5, 2000, &[&["a", &address]]);
    storage.persist(&forged).await.unwrap();
    assert_eq!(query_kind(&storage, 30009).len(), 1);
    assert_eq!(storage.deleted_count(), 0);

    // The author's own deletion hides it.
    let deletion = build_parsed_worker_message(&hex_id(3), &author, 5, 2500, &[&["a", &address]]);
    storage.persist(&deletion).await.unwrap();
    assert!(query_kind(&storage, 30009).is_empty());
    assert_eq!(storage.deleted_count(), 1);

    // Rebuilding from the shard snapshot re-applies the deletion, because
    // re-indexing funnels through the same ingest path.
    storage.rebuild_indexes_from_storage().unwrap();
    assert!(query_kind(&storage, 30009).is_empty());
    assert_eq!(storage.deleted_count(), 1);
}
