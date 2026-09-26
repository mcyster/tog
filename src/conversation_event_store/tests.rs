use std::io::Write;
use std::path::PathBuf;

use schemars::json_schema;
use serde_json::json;

use super::{
    ConversationEventStore, ConversationStoreError, ConversationStoreLoadError, FileEventStore, log,
};
use crate::conversation::{
    Conversation, ConversationEvent, ConversationEventId, ConversationEventRecord,
    ConversationHistory, ConversationId, ConversationView, ModelData, ToolOutcome, ToolRequest,
    ToolResponse, User, UserContent,
};
use crate::toolset::{ToolDefinition, ToolName, Toolset};

fn temporary_store() -> FileEventStore {
    let directory = std::env::temp_dir().join(format!("tog-test-{}", uuid::Uuid::now_v7()));
    FileEventStore::new(directory).expect("the event store should be created")
}

fn conversation_event_log_path(store: &FileEventStore, conversation_id: ConversationId) -> PathBuf {
    log::log_path(&store.conversation_directory(conversation_id))
}

fn user_event(content: &str) -> ConversationEvent {
    ConversationEvent::User(
        User::new(vec![UserContent::Text(content.to_owned())])
            .expect("the user event should be valid"),
    )
}

fn user_record(
    conversation_id: ConversationId,
    position: u64,
    content: &str,
) -> ConversationEventRecord {
    ConversationEventRecord::new(conversation_id, position, user_event(content))
}

fn tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition::try_new(
        ToolName::try_new(name.to_owned()).expect("the tool name should be valid"),
        "Run a command.".to_owned(),
        json_schema!({ "type": "object" }),
        json_schema!({ "type": "object" }),
    )
    .expect("the tool definition should be valid")
}

fn tool_request_event(model_request_id: ConversationEventId, arguments: &str) -> ConversationEvent {
    ConversationEvent::ToolRequest(
        ToolRequest::try_new(
            model_request_id,
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            serde_json::from_str(arguments).expect("the arguments should be JSON"),
            None,
        )
        .expect("the tool request should be valid"),
    )
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
        .append(conversation_id, vec![user_event("first")])
        .expect("the first event should be persisted");
    let second_batch = store
        .append(conversation_id, vec![user_event("second")])
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
            vec![
                user_event("first"),
                user_event("second"),
                user_event("third"),
            ],
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
        user_record(conversation_id, 0, "first"),
        user_record(conversation_id, 1, "second"),
    ];
    events[1].position = 2;
    std::fs::create_dir_all(store.conversation_directory(conversation_id))
        .expect("the conversation directory should be created");
    std::fs::write(
        conversation_event_log_path(&store, conversation_id),
        log::encode_batch(&events).expect("the batch should encode"),
    )
    .expect("the log should be written");

    let error = store
        .load(conversation_id)
        .expect_err("the incomplete log should be rejected");
    assert!(matches!(
        error,
        ConversationStoreLoadError::Store(ConversationStoreError::CorruptData)
    ));
    assert_eq!(error.to_string(), "corrupt conversation data");
    assert!(
        store
            .append(conversation_id, vec![user_event("third")])
            .is_err()
    );
}

#[test]
fn recovery_ignores_an_incomplete_trailing_batch() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    store
        .append(conversation_id, vec![user_event("committed")])
        .expect("the committed event should be persisted");
    let log_path = conversation_event_log_path(&store, conversation_id);
    let torn_batch = log::encode_batch(&[user_record(conversation_id, 1, "torn")])
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
        .append(conversation_id, vec![user_event("after")])
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
        .append(conversation_id, vec![user_event("committed")])
        .expect("the committed event should be persisted");
    let log_path = conversation_event_log_path(&store, conversation_id);
    let mut uncommitted = Vec::new();
    uncommitted.extend_from_slice(b"{\"transaction\":\"begin\"}\n");
    let event = user_record(conversation_id, 1, "uncommitted");
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
        .append(conversation_id, vec![user_event("after")])
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
        .append(conversation_id, vec![user_event("first")])
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
        ConversationStoreLoadError::Store(ConversationStoreError::CorruptData)
    ));
}

#[test]
fn event_store_round_trips_tool_definitions_requests_and_responses() {
    use crate::conversation::{
        ModelId, ModelRequest, ModelSource, ProviderId, TurnEnd, TurnOutcome, TurnStart,
    };
    use std::str::FromStr;
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    let tool_definition = tool_definition("shell");

    let user_records = store
        .append(conversation_id, vec![user_event("run pwd")])
        .expect("the user event should persist");
    let turn_start = store
        .append(
            conversation_id,
            vec![ConversationEvent::TurnStart(TurnStart::new(
                Some(user_records[0].id),
                0,
            ))],
        )
        .expect("the turn start should persist");
    let turn_id = turn_start[0].id;
    let model_request = store
        .append(
            conversation_id,
            vec![ConversationEvent::ModelRequest(
                ModelRequest::new(
                    turn_id,
                    ModelSource::new(
                        ProviderId::from_str("test").expect("the provider should be valid"),
                        ModelId::from_str("model").expect("the model should be valid"),
                    ),
                    1,
                    Vec::new(),
                    None,
                    None,
                )
                .expect("the model request should be valid"),
            )],
        )
        .expect("the model request should persist");
    let model_request_id = model_request[0].id;
    let tool_request_event = tool_request_event(model_request_id, r#"{ "command": "pwd" }"#);
    let records = store
        .append(
            conversation_id,
            vec![
                ConversationEvent::Toolset(
                    Toolset::immediate(vec![tool_definition.clone()])
                        .expect("the toolset should be valid"),
                ),
                tool_request_event.clone(),
            ],
        )
        .expect("the tool definitions and request should persist");
    let tool_request_id = records[1].id;
    store
        .append(
            conversation_id,
            vec![
                ConversationEvent::ToolResponse(ToolResponse::new(
                    tool_request_id,
                    ToolOutcome::succeeded(json!({ "stdout": "/tmp\n" })),
                )),
                ConversationEvent::ModelResponse(
                    crate::conversation::ModelResponse::new(
                        model_request_id,
                        vec![tool_request_id],
                        crate::conversation::ModelOutcome::Succeeded,
                        None,
                    )
                    .expect("the model response should be valid"),
                ),
                ConversationEvent::TurnEnd(
                    TurnEnd::new(turn_id, TurnOutcome::Succeeded)
                        .expect("the turn end should be valid"),
                ),
            ],
        )
        .expect("the tool response and turn end should persist");

    let events = store
        .load(conversation_id)
        .expect("the conversation should load");
    let conversation =
        ConversationHistory::from_events(events).expect("the stored events should reconstruct");

    let view = ConversationView::new(&conversation);
    let toolset = view.toolset().expect("the toolset should be declared");
    assert_eq!(toolset.entries().len(), 1);
    assert_eq!(toolset.entries()[0].definition().clone(), tool_definition);
    assert!(matches!(
        &conversation.events()[4].event,
        ConversationEvent::ToolRequest(request) if request == &tool_request_payload(&tool_request_event)
    ));
    assert!(matches!(
        &conversation.events()[5].event,
        ConversationEvent::ToolResponse(response)
            if response.tool_request_id() == tool_request_id
    ));
}

fn tool_request_payload(event: &ConversationEvent) -> crate::conversation::ToolRequest {
    let ConversationEvent::ToolRequest(request) = event else {
        panic!("the event should be a tool request");
    };
    request.clone()
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
    std::fs::write(
        conversation_event_log_path(store, event.conversation_id),
        log::encode_batch(&events).expect("the batch should encode"),
    )
    .expect("the event timestamp should be written");
}

#[test]
fn latest_conversation_is_the_most_recently_active_one() {
    let store = temporary_store();
    let first_conversation_id = ConversationId::new();
    let second_conversation_id = ConversationId::new();
    let first_batch = store
        .append(first_conversation_id, vec![user_event("first")])
        .expect("the first event should be persisted");
    let second_batch = store
        .append(second_conversation_id, vec![user_event("second")])
        .expect("the second event should be persisted");
    set_event_timestamp(&store, &first_batch[0], timestamp(1));
    set_event_timestamp(&store, &second_batch[0], timestamp(2));

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be found"),
        Some(second_conversation_id)
    );

    let third_batch = store
        .append(first_conversation_id, vec![user_event("third")])
        .expect("the third event should be persisted");
    set_event_timestamp(&store, &third_batch[0], timestamp(3));

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be found"),
        Some(first_conversation_id)
    );
}

#[test]
fn latest_conversation_is_absent_without_conversations() {
    let store = temporary_store();

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be absent"),
        None
    );
}

#[test]
fn latest_conversation_ignores_conversations_without_events() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    store
        .append(conversation_id, vec![user_event("first")])
        .expect("the event should be persisted");
    let empty_conversation_id = ConversationId::new();
    std::fs::create_dir_all(store.conversation_directory(empty_conversation_id))
        .expect("the empty conversation directory should be created");

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be found"),
        Some(conversation_id)
    );
}

#[test]
fn model_data_round_trips_an_opaque_payload() {
    let model_data = ModelData::new(
        [("call_id".to_owned(), json!("call_native"))]
            .into_iter()
            .collect(),
    )
    .expect("the model data should be valid");

    let restored: ModelData = serde_json::from_value(
        serde_json::to_value(&model_data).expect("the data should serialize"),
    )
    .expect("the data should deserialize");
    assert_eq!(restored, model_data);
    assert_eq!(restored.content()["call_id"], "call_native");
}
