use std::io::Write;
use std::path::PathBuf;

use schemars::json_schema;
use serde_json::json;

use super::{
    ConversationEventRecord, ConversationEventStore, ConversationStoreError,
    ConversationStoreLoadError, FileEventStore, log,
};
use crate::conversation::{
    Conversation, ConversationEvent, ConversationEventId, ConversationEventPayload,
    ConversationHistory, ConversationId, ConversationView, ModelData, ToolDefinition, ToolName,
    ToolOutcome, ToolRequest, ToolResponse, User, UserContent,
};
use crate::toolset::Toolset;

fn temporary_store() -> FileEventStore {
    let directory = std::env::temp_dir().join(format!("tog-test-{}", uuid::Uuid::now_v7()));
    FileEventStore::new(directory).expect("the event store should be created")
}

fn conversation_event_log_path(store: &FileEventStore, conversation_id: ConversationId) -> PathBuf {
    log::log_path(&store.conversation_directory(conversation_id))
}

fn user_event(conversation_id: ConversationId, content: &str) -> ConversationEvent {
    ConversationEvent::new(
        conversation_id,
        ConversationEventPayload::User(
            User::new(vec![UserContent::Text(content.to_owned())])
                .expect("the user event should be valid"),
        ),
    )
}

fn user_record(
    conversation_id: ConversationId,
    position: u64,
    content: &str,
) -> ConversationEventRecord {
    ConversationEventRecord::new(position, user_event(conversation_id, content))
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

fn tool_request_event(
    model_request_id: ConversationEventId,
    arguments: &str,
) -> ConversationEventPayload {
    ConversationEventPayload::ToolRequest(
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
        .append(conversation_id, vec![user_event(conversation_id, "first")])
        .expect("the first event should be persisted");
    let second_batch = store
        .append(conversation_id, vec![user_event(conversation_id, "second")])
        .expect("the second event should be persisted");
    let first_event = &first_batch[0];
    let second_event = &second_batch[0];

    assert_eq!(first_event.event.conversation_id(), conversation_id);
    assert_eq!(first_event.position, 0);
    assert_ne!(
        first_event.event.timestamp(),
        time::OffsetDateTime::UNIX_EPOCH
    );
    assert_eq!(second_event.event.conversation_id(), conversation_id);
    assert_eq!(second_event.position, 1);
    assert_ne!(
        second_event.event.timestamp(),
        time::OffsetDateTime::UNIX_EPOCH
    );
    assert_ne!(second_event.event.id(), first_event.event.id());

    let events = store
        .load(conversation_id)
        .expect("the conversation should load");
    assert_eq!(events[0].position, 0);
    assert_eq!(events[1].position, 1);
    assert!(
        events
            .iter()
            .all(|event| event.event.conversation_id() == conversation_id)
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
fn event_identity_and_timestamp_survive_append_and_load() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();

    let appended = store
        .append(conversation_id, vec![user_event(conversation_id, "plain")])
        .expect("the event should be persisted");
    let loaded = store
        .load(conversation_id)
        .expect("the conversation should load");

    assert_eq!(loaded, appended);
    assert_eq!(loaded[0].event.id(), appended[0].event.id());
    assert_eq!(loaded[0].event.timestamp(), appended[0].event.timestamp());
    assert_eq!(loaded[0].event.conversation_id(), conversation_id);
}

#[test]
fn appending_an_event_batch_commits_its_events_in_order() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();

    let appended = store
        .append(
            conversation_id,
            vec![
                user_event(conversation_id, "first"),
                user_event(conversation_id, "second"),
                user_event(conversation_id, "third"),
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
            .append(conversation_id, vec![user_event(conversation_id, "third")])
            .is_err()
    );
}

#[test]
fn recovery_ignores_an_incomplete_trailing_batch() {
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    store
        .append(
            conversation_id,
            vec![user_event(conversation_id, "committed")],
        )
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
        .append(conversation_id, vec![user_event(conversation_id, "after")])
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
        .append(
            conversation_id,
            vec![user_event(conversation_id, "committed")],
        )
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
        .append(conversation_id, vec![user_event(conversation_id, "after")])
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
        .append(conversation_id, vec![user_event(conversation_id, "first")])
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
        ModelId, ModelOutcome, ModelRequest, ModelResponse, ModelSource, ProviderId, TurnEnd,
        TurnOutcome, TurnStart,
    };
    use std::str::FromStr;
    let store = temporary_store();
    let conversation_id = ConversationId::new();
    let tool_definition = tool_definition("shell");

    let user_records = store
        .append(
            conversation_id,
            vec![user_event(conversation_id, "run pwd")],
        )
        .expect("the user event should persist");
    let turn_start = store
        .append(
            conversation_id,
            vec![ConversationEvent::new(
                conversation_id,
                ConversationEventPayload::TurnStart(TurnStart::new(
                    Some(user_records[0].event.id()),
                    0,
                )),
            )],
        )
        .expect("the turn start should persist");
    let turn_id = turn_start[0].event.id();
    let model_request = store
        .append(
            conversation_id,
            vec![ConversationEvent::new(
                conversation_id,
                ConversationEventPayload::ModelRequest(
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
                ),
            )],
        )
        .expect("the model request should persist");
    let model_request_id = model_request[0].event.id();
    let tool_request_event = tool_request_event(model_request_id, r#"{ "command": "pwd" }"#);
    let records = store
        .append(
            conversation_id,
            vec![
                ConversationEvent::new(
                    conversation_id,
                    ConversationEventPayload::Tools(
                        Toolset::immediate(vec![tool_definition.clone()]).into_tools(),
                    ),
                ),
                ConversationEvent::new(conversation_id, tool_request_event.clone()),
            ],
        )
        .expect("the tool definitions and request should persist");
    let tool_request_id = records[1].event.id();
    store
        .append(
            conversation_id,
            vec![
                ConversationEvent::new(
                    conversation_id,
                    ConversationEventPayload::ToolResponse(ToolResponse::new(
                        tool_request_id,
                        ToolOutcome::succeeded(json!({ "stdout": "/tmp\n" })),
                    )),
                ),
                ConversationEvent::new(
                    conversation_id,
                    ConversationEventPayload::ModelResponse(
                        ModelResponse::new(
                            model_request_id,
                            vec![tool_request_id],
                            ModelOutcome::Succeeded,
                            None,
                        )
                        .expect("the model response should be valid"),
                    ),
                ),
                ConversationEvent::new(
                    conversation_id,
                    ConversationEventPayload::TurnEnd(
                        TurnEnd::new(turn_id, TurnOutcome::Succeeded)
                            .expect("the turn end should be valid"),
                    ),
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
    let tools = view.tools().expect("the tools should be declared");
    assert_eq!(tools.tools().len(), 1);
    assert_eq!(tools.tools()[0].definition().clone(), tool_definition);
    let recorded_tool_request = conversation.events()[4].event.payload();
    assert!(matches!(
        recorded_tool_request,
        ConversationEventPayload::ToolRequest(request) if request == &tool_request_payload(&tool_request_event)
    ));
    let tool_response = conversation.events()[5].event.payload();
    assert!(matches!(
        tool_response,
        ConversationEventPayload::ToolResponse(response)
            if response.tool_request_id() == tool_request_id
    ));
}

fn tool_request_payload(content: &ConversationEventPayload) -> crate::conversation::ToolRequest {
    let ConversationEventPayload::ToolRequest(request) = content else {
        panic!("the content should be a tool request");
    };
    request.clone()
}

fn timestamp(event: ConversationEvent, day: u64) -> ConversationEvent {
    ConversationEvent::at(
        event.conversation_id(),
        event.id(),
        time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(day as i64),
        event.payload().clone(),
    )
}

#[test]
fn latest_conversation_is_the_most_recently_active_one() {
    let store = temporary_store();
    let first_conversation_id = ConversationId::new();
    let second_conversation_id = ConversationId::new();
    store
        .append(
            first_conversation_id,
            vec![timestamp(user_event(first_conversation_id, "first"), 1)],
        )
        .expect("the first event should be persisted");
    store
        .append(
            second_conversation_id,
            vec![timestamp(user_event(second_conversation_id, "second"), 2)],
        )
        .expect("the second event should be persisted");

    assert_eq!(
        store
            .latest_id()
            .expect("the latest conversation should be found"),
        Some(second_conversation_id)
    );

    store
        .append(
            first_conversation_id,
            vec![timestamp(user_event(first_conversation_id, "third"), 3)],
        )
        .expect("the third event should be persisted");

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
        .append(conversation_id, vec![user_event(conversation_id, "first")])
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
