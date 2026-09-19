use std::io::Write;
use std::path::{Path, PathBuf};

use schemars::json_schema;
use serde_json::{Map, Value, json};

use super::{ConversationEventStore, ConversationEventStoreError, FileEventStore, legacy, log};
use crate::conversation::{
    AssistantResponse, Conversation, ConversationCommandId, ConversationEvent,
    ConversationEventKind, ConversationEventRecord, ConversationFact, ConversationId,
    ConversationMessage, ConversationTurnId, ModelData, ModelInvocationId, ToolCallId,
    ToolDefinition, ToolName, ToolOutcome, ToolRequest, ToolResponse, UserContent,
};

fn temporary_store() -> FileEventStore {
    let directory = std::env::temp_dir().join(format!("tog-test-{}", uuid::Uuid::now_v7()));
    FileEventStore::new(directory).expect("the event store should be created")
}

fn conversation_event_log_path(store: &FileEventStore, conversation_id: ConversationId) -> PathBuf {
    log::log_path(&store.conversation_directory(conversation_id))
}

fn write_legacy_event(directory: &Path, event: &ConversationEventRecord, identifier: &str) {
    let path = directory.join(format!("{:020}-{identifier}.json", event.position));
    let mut contents = serde_json::to_vec(event).expect("the legacy event should serialize");
    contents.push(b'\n');
    log::write_file_atomically(&path, &contents).expect("the legacy event should be written");
}

fn user_message_fact(content: &str) -> ConversationFact {
    ConversationFact::Message {
        message: ConversationMessage::User {
            caused_by: Some(ConversationCommandId::new()),
            content: vec![UserContent::Text(content.to_owned())],
        },
        turn_id: None,
    }
}

fn user_fact(content: &str) -> ConversationEvent {
    ConversationEvent::Fact(user_message_fact(content))
}

fn user_kind(content: &str) -> ConversationEventKind {
    ConversationEventKind::Fact(user_message_fact(content))
}

#[test]
fn crc32_matches_the_standard_check_value() {
    assert_eq!(log::crc32(b"123456789"), 0xcbf4_3926);
}

#[test]
fn event_store_assigns_canonical_envelope_metadata() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();

    let first_batch = store
        .append(conversation_id, vec![user_fact("first")])
        .expect("the first event should be persisted");
    let second_batch = store
        .append(conversation_id, vec![user_fact("second")])
        .expect("the second event should be persisted");
    let first_event = &first_batch[0];
    let second_event = &second_batch[0];

    assert_eq!(first_event.conversation_id, conversation_id);
    assert_eq!(first_event.position, 0);
    assert_eq!(first_event.schema_version, 13);
    assert_ne!(first_event.timestamp, time::OffsetDateTime::UNIX_EPOCH);
    assert_eq!(second_event.conversation_id, conversation_id);
    assert_eq!(second_event.position, 1);
    assert_eq!(second_event.schema_version, 13);
    assert_ne!(second_event.timestamp, time::OffsetDateTime::UNIX_EPOCH);
    assert_ne!(second_event.id, first_event.id);

    let events = store
        .load(conversation_id)
        .expect("the conversation should load");
    assert_eq!(events[0].position, 0);
    assert_eq!(events[1].position, 1);
    assert!(
        events
            .iter()
            .all(|event| event.conversation_id == conversation_id)
    );
    assert!(
        !store
            .conversation_directory(conversation_id)
            .join("conversation.json")
            .exists()
    );
    assert!(conversation_event_log_path(&store, conversation_id).exists());
}

#[test]
fn appending_an_event_batch_commits_its_events_in_order() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();

    let appended = store
        .append(
            conversation_id,
            vec![user_fact("first"), user_fact("second"), user_fact("third")],
        )
        .expect("the batch should be persisted");

    assert_eq!(
        appended
            .iter()
            .map(|event| event.position)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let loaded = store.load(conversation_id).expect("the log should load");
    assert_eq!(loaded, appended);
}

#[test]
fn event_store_rejects_committed_positions_with_a_gap() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    let mut events = vec![
        ConversationEventRecord::new(conversation_id, 0, user_kind("first")),
        ConversationEventRecord::new(conversation_id, 1, user_kind("second")),
    ];
    events[1].position = 2;
    std::fs::create_dir_all(store.conversation_directory(conversation_id))
        .expect("the conversation directory should be created");
    log::write_file_atomically(
        &conversation_event_log_path(&store, conversation_id),
        &log::encode_batch(&events).expect("the batch should encode"),
    )
    .expect("the log should be written");

    let error = store
        .load(conversation_id)
        .expect_err("the incomplete log should be rejected");
    assert!(matches!(
        error,
        ConversationEventStoreError::Storage(ref storage_error)
            if storage_error.kind() == std::io::ErrorKind::InvalidData
    ));
    assert_eq!(
        error.to_string(),
        "expected conversation event position 1, found 2"
    );
    assert!(
        store
            .append(conversation_id, vec![user_fact("third")])
            .is_err()
    );
}

#[test]
fn recovery_ignores_an_incomplete_trailing_batch() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    store
        .append(conversation_id, vec![user_fact("committed")])
        .expect("the committed event should be persisted");
    let log_path = conversation_event_log_path(&store, conversation_id);
    let torn_batch = log::encode_batch(&[ConversationEventRecord::new(
        conversation_id,
        1,
        user_kind("torn"),
    )])
    .expect("the torn batch should encode");
    let mut log_file = std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("the log should open");
    log_file
        .write_all(&torn_batch[..torn_batch.len() - 8])
        .expect("the torn tail should be written");

    let loaded = store
        .load(conversation_id)
        .expect("the committed log should load");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].position, 0);

    store
        .append(conversation_id, vec![user_fact("after")])
        .expect("the torn tail should be discarded");
    let loaded = store.load(conversation_id).expect("the log should load");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[1].position, 1);
}

#[test]
fn recovery_ignores_a_complete_transaction_without_a_commit_marker() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    store
        .append(conversation_id, vec![user_fact("committed")])
        .expect("the committed event should be persisted");
    let log_path = conversation_event_log_path(&store, conversation_id);
    let mut uncommitted = Vec::new();
    uncommitted.extend_from_slice(b"{\"transaction\":\"begin\"}\n");
    let event = ConversationEventRecord::new(conversation_id, 1, user_kind("uncommitted"));
    serde_json::to_writer(&mut uncommitted, &event).expect("the event should encode");
    uncommitted.push(b'\n');
    let mut log_file = std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("the log should open");
    log_file
        .write_all(&uncommitted)
        .expect("the uncommitted transaction should be written");

    let loaded = store
        .load(conversation_id)
        .expect("the committed log should load");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].position, 0);

    store
        .append(conversation_id, vec![user_fact("after")])
        .expect("the uncommitted transaction should be discarded");
    let loaded = store.load(conversation_id).expect("the log should load");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[1].position, 1);
}

#[test]
fn corruption_inside_committed_history_is_rejected() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    store
        .append(conversation_id, vec![user_fact("first")])
        .expect("the event should be persisted");
    let log_path = conversation_event_log_path(&store, conversation_id);
    let mut bytes = std::fs::read(&log_path).expect("the log should be readable");
    let content_offset = bytes
        .windows(b"first".len())
        .position(|window| window == b"first")
        .expect("the event content should be present");
    bytes[content_offset] = b'F';
    std::fs::write(&log_path, &bytes).expect("the corrupted log should be written");

    let error = store
        .load(conversation_id)
        .expect_err("corruption inside committed history should be rejected");
    assert!(matches!(
        error,
        ConversationEventStoreError::Storage(ref storage_error)
            if storage_error.kind() == std::io::ErrorKind::InvalidData
    ));
}

#[test]
fn event_store_persists_model_data_on_the_event_envelope() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    let model_data = ModelData::new(Map::from_iter([(
        "response_id".to_owned(),
        Value::String("resp_1".to_owned()),
    )]))
    .expect("the model data should be valid");

    let assistant = ConversationEvent::Fact(ConversationFact::Message {
        message: ConversationMessage::AssistantResponse {
            invocation_id: ModelInvocationId::new(),
            data: Some(model_data),
            response: AssistantResponse::new("The answer is 42.".to_owned())
                .expect("the assistant response should be valid"),
        },
        turn_id: Some(ConversationTurnId::new()),
    });
    let batch = store
        .append(conversation_id, vec![assistant])
        .expect("the model event should be persisted");

    let loaded = store
        .load(conversation_id)
        .expect("the conversation should load");
    assert_eq!(loaded[0], batch[0]);
}

#[test]
fn event_store_round_trips_tool_definitions_requests_and_responses() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    let call_id = ToolCallId::new();
    let turn_id = ConversationTurnId::new();
    let tool_definition = ToolDefinition::try_new(
        ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
        "Run a command.".to_owned(),
        json_schema!({ "type": "object" }),
        json_schema!({ "type": "object" }),
    )
    .expect("the tool definition should be valid");
    let request = ToolRequest::try_new(
        call_id,
        ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
        json!({ "command": "pwd" }),
        ModelInvocationId::new(),
        None,
    )
    .expect("the tool request should be valid");
    let response = ToolResponse::new(
        call_id,
        ToolOutcome::Result {
            value: json!({ "stdout": "/tmp\n" }),
        },
    );

    store
        .append(
            conversation_id,
            vec![ConversationEvent::Fact(ConversationFact::ToolsAvailable {
                tools: vec![tool_definition.clone()],
            })],
        )
        .expect("the tool definitions should persist");
    store
        .append(
            conversation_id,
            vec![ConversationEvent::Fact(ConversationFact::ToolRequest {
                request: request.clone(),
                turn_id: Some(turn_id),
            })],
        )
        .expect("the tool request should persist");
    store
        .append(
            conversation_id,
            vec![ConversationEvent::Fact(ConversationFact::ToolResponse {
                response: response.clone(),
                turn_id: Some(turn_id),
            })],
        )
        .expect("the tool response should persist");

    let events = store
        .load(conversation_id)
        .expect("the conversation should load");
    let conversation =
        Conversation::from_events(events).expect("the stored events should reconstruct");

    assert_eq!(conversation.available_tools(), [tool_definition]);
    assert!(matches!(
        &conversation.events()[1].kind,
        crate::conversation::StoredConversationEventKind::Shared(
            crate::conversation::ConversationEventKind::Fact(
                ConversationFact::ToolRequest { request: restored, turn_id: Some(restored_turn) }
            )
        ) if restored == &request && restored_turn == &turn_id
    ));
    assert!(matches!(
        &conversation.events()[2].kind,
        crate::conversation::StoredConversationEventKind::Shared(
            crate::conversation::ConversationEventKind::Fact(
                ConversationFact::ToolResponse { response: restored, .. }
            )
        ) if restored == &response
    ));
}

#[test]
fn event_store_migrates_legacy_per_event_files_on_append() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    let conversation_directory = store.conversation_directory(conversation_id);
    let legacy_directory = legacy::events_directory(&conversation_directory);
    std::fs::create_dir_all(&legacy_directory).expect("the legacy directory should be created");
    let first = ConversationEventRecord::new(conversation_id, 0, user_kind("first"));
    let second = ConversationEventRecord::new(conversation_id, 1, user_kind("second"));
    write_legacy_event(&legacy_directory, &first, "first");
    write_legacy_event(&legacy_directory, &second, "second");

    let loaded = store
        .load(conversation_id)
        .expect("the legacy events should load");
    assert_eq!(loaded, vec![first.clone(), second.clone()]);

    let appended = store
        .append(conversation_id, vec![user_fact("third")])
        .expect("the new event should migrate the legacy log");
    assert_eq!(appended[0].position, 2);

    let loaded = store
        .load(conversation_id)
        .expect("the migrated log should load");
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0], first);
    assert_eq!(loaded[1], second);
    assert!(!legacy_directory.exists());
    assert!(conversation_event_log_path(&store, conversation_id).exists());
}

fn timestamp(day: u64) -> time::OffsetDateTime {
    time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(day as i64)
}

fn set_event_timestamp(
    store: &FileEventStore,
    event: &ConversationEventRecord,
    timestamp: time::OffsetDateTime,
) {
    let mut events = store
        .load(event.conversation_id)
        .expect("the log should load");
    for loaded_event in &mut events {
        if loaded_event.id == event.id {
            loaded_event.timestamp = timestamp;
        }
    }
    log::write_file_atomically(
        &conversation_event_log_path(store, event.conversation_id),
        &log::encode_batch(&events).expect("the batch should encode"),
    )
    .expect("the event timestamp should be written");
}

#[test]
fn latest_conversation_is_the_most_recently_active_one() {
    let store = temporary_store();
    let first_conversation_id = ConversationId::new();
    let second_conversation_id = ConversationId::new();
    let first_batch = store
        .append(first_conversation_id, vec![user_fact("first")])
        .expect("the first event should be persisted");
    let second_batch = store
        .append(second_conversation_id, vec![user_fact("second")])
        .expect("the second event should be persisted");
    set_event_timestamp(&store, &first_batch[0], timestamp(1));
    set_event_timestamp(&store, &second_batch[0], timestamp(2));

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be found"),
        second_conversation_id
    );

    let third_batch = store
        .append(first_conversation_id, vec![user_fact("third")])
        .expect("the third event should be persisted");
    set_event_timestamp(&store, &third_batch[0], timestamp(3));

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be found"),
        first_conversation_id
    );
}

#[test]
fn latest_conversation_is_absent_without_conversations() {
    let store = temporary_store();

    let error = store
        .latest_id()
        .expect_err("the latest conversation should be missing");

    assert!(matches!(
        error,
        ConversationEventStoreError::NoConversations
    ));
    assert_eq!(error.to_string(), "no conversations found");
}

#[test]
fn latest_conversation_ignores_conversations_without_events() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    store
        .append(conversation_id, vec![user_fact("first")])
        .expect("the event should be persisted");
    let empty_conversation_id = ConversationId::new();
    std::fs::create_dir_all(store.conversation_directory(empty_conversation_id))
        .expect("the empty conversation directory should be created");

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be found"),
        conversation_id
    );
}

#[test]
fn latest_conversation_ignores_an_earlier_schema() {
    let store = temporary_store();
    let legacy_conversation_id = ConversationId::new();
    let legacy_directory =
        legacy::events_directory(&store.conversation_directory(legacy_conversation_id));
    std::fs::create_dir_all(&legacy_directory).expect("the legacy directory should be created");
    std::fs::write(
        legacy_directory.join("00000000000000000000-01a00692c0dc7402a70f67ae862a5eb5.json"),
        concat!(
            r#"{"position":0,"id":"01a00692-c0dc-7402-a70f-67ae862a5eb5","#,
            r#""timestamp_milliseconds":1786816676060,"schema_version":1,"#,
            r#""event":{"type":"user","text":"test"}}"#
        ),
    )
    .expect("the earlier schema event should be written");

    let error = store
        .latest_id()
        .expect_err("the earlier schema should not count as a conversation");
    assert!(matches!(
        error,
        ConversationEventStoreError::NoConversations
    ));

    let current_conversation_id = ConversationId::new();
    store
        .append(current_conversation_id, vec![user_fact("current")])
        .expect("the event should be persisted");

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be found"),
        current_conversation_id
    );
}
