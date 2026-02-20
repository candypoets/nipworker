use flatbuffers::FlatBufferBuilder;
use shared::generated::nostr::fb;

/// Serialize a ConnectionStatus WorkerMessage to bytes for batch buffer.
/// Returns the serialized FlatBuffer bytes.
pub fn serialize_connection_status(relay_url: &str, status: &str, message: &str) -> Vec<u8> {
    let mut builder = FlatBufferBuilder::new();

    let relay_url_offset = builder.create_string(relay_url);
    let status_offset = builder.create_string(status);
    let message_offset = builder.create_string(message);

    let conn_status_args = fb::ConnectionStatusArgs {
        relay_url: Some(relay_url_offset),
        status: Some(status_offset),
        message: Some(message_offset),
    };
    let conn_status_offset = fb::ConnectionStatus::create(&mut builder, &conn_status_args);

    let message_args = fb::WorkerMessageArgs {
        sub_id: None,
        url: None,
        type_: fb::MessageType::ConnectionStatus,
        content_type: fb::Message::ConnectionStatus,
        content: Some(conn_status_offset.as_union_value()),
    };
    let root = fb::WorkerMessage::create(&mut builder, &message_args);
    builder.finish(root, None);

    builder.finished_data().to_vec()
}

/// Serialize an EOCE WorkerMessage to bytes for batch buffer.
/// Returns the serialized FlatBuffer bytes.
pub fn serialize_eoce() -> Vec<u8> {
    let mut builder = FlatBufferBuilder::new();

    let subscription_id = builder.create_string("");
    let eoce_args = fb::EoceArgs {
        subscription_id: Some(subscription_id),
    };
    let eoce_offset = fb::Eoce::create(&mut builder, &eoce_args);

    let message_args = fb::WorkerMessageArgs {
        sub_id: None,
        url: None,
        type_: fb::MessageType::Eoce,
        content_type: fb::Message::Eoce,
        content: Some(eoce_offset.as_union_value()),
    };
    let root = fb::WorkerMessage::create(&mut builder, &message_args);
    builder.finish(root, None);

    builder.finished_data().to_vec()
}
