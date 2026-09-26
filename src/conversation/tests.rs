use schemars::json_schema;
use serde_json::json;
use std::str::FromStr;
use time::OffsetDateTime;

use super::history::InvalidConversation;
use super::{Conversation, ConversationHistory, ConversationId, ConversationView};
use crate::conversation::{
    AssistantResponse, Context, ConversationEvent, ConversationEventId, ConversationEventRecord,
    FailureCategory, ModelId, ModelOutcome, ModelRequest, ModelResponse, ModelSource,
    ModelSpecificEvent, OperationFailure, ProviderId, ToolDefinition, ToolName, ToolOutcome,
    ToolRequest, ToolResponse, TurnEnd, TurnOutcome, TurnStart, User, UserContent,
};

fn tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition::try_new(
        ToolName::try_new(name.to_owned()).expect("the tool name should be valid"),
        "Run something.".to_owned(),
        json_schema!({ "type": "object" }),
        json_schema!({ "type": "object" }),
    )
    .expect("the tool definition should be valid")
}

fn record(
    conversation_id: ConversationId,
    position: u64,
    id: ConversationEventId,
    event: ConversationEvent,
) -> ConversationEventRecord {
    ConversationEventRecord {
        conversation_id,
        position,
        id,
        timestamp: OffsetDateTime::UNIX_EPOCH,
        schema_version: 13,
        event,
    }
}

fn user_event(conversation_id: ConversationId, position: u64) -> ConversationEventRecord {
    record(
        conversation_id,
        position,
        ConversationEventId::new(),
        ConversationEvent::User(
            User::new(vec![UserContent::Text(format!("event {position}"))])
                .expect("the user event should be valid"),
        ),
    )
}

fn tools_available_event(
    conversation_id: ConversationId,
    position: u64,
    tools: Vec<ToolDefinition>,
) -> ConversationEventRecord {
    record(
        conversation_id,
        position,
        ConversationEventId::new(),
        ConversationEvent::Context(Context::tools_available(tools)),
    )
}

fn source() -> ModelSource {
    ModelSource::new(
        ProviderId::from_str("test").expect("the provider should be valid"),
        ModelId::from_str("test-model").expect("the model should be valid"),
    )
}

#[test]
fn conversation_requires_an_event() {
    assert_eq!(
        ConversationHistory::from_events(Vec::new()),
        Err(InvalidConversation::Empty)
    );
}

#[test]
fn conversation_rejects_mixed_conversation_ids() {
    let first_conversation_id = ConversationId::new();
    let second_conversation_id = ConversationId::new();

    let result = ConversationHistory::from_events(vec![
        user_event(first_conversation_id, 0),
        user_event(second_conversation_id, 1),
    ]);

    assert_eq!(
        result,
        Err(InvalidConversation::MixedConversationIds {
            expected: first_conversation_id,
            found: second_conversation_id,
        })
    );
}

#[test]
fn conversation_rejects_invalid_event_order() {
    let conversation_id = ConversationId::new();

    let result = ConversationHistory::from_events(vec![
        user_event(conversation_id, 1),
        user_event(conversation_id, 1),
    ]);

    assert_eq!(
        result,
        Err(InvalidConversation::InvalidPosition {
            expected: 2,
            found: 1,
        })
    );
}

#[test]
fn conversation_exposes_its_id_and_ordered_events() {
    let conversation_id = ConversationId::new();

    let conversation = ConversationHistory::from_events(vec![
        user_event(conversation_id, 0),
        user_event(conversation_id, 1),
    ])
    .expect("the conversation should be valid");

    assert_eq!(conversation.id(), conversation_id);
    assert_eq!(conversation.events().len(), 2);
    assert_eq!(conversation.events()[0].position, 0);
    assert_eq!(conversation.events()[1].position, 1);
}

#[test]
fn available_tools_uses_the_latest_declaration() {
    let conversation_id = ConversationId::new();

    let conversation = ConversationHistory::from_events(vec![
        tools_available_event(conversation_id, 0, vec![tool_definition("first")]),
        user_event(conversation_id, 1),
        tools_available_event(conversation_id, 2, vec![tool_definition("second")]),
    ])
    .expect("the conversation should be valid");

    let view = ConversationView::new(&conversation);
    let available_tools = view.available_tools();
    assert_eq!(available_tools.len(), 1);
    assert_eq!(available_tools[0].name().as_str(), "second");
}

#[test]
fn an_empty_tools_available_declaration_removes_all_tools() {
    let conversation_id = ConversationId::new();

    let conversation = ConversationHistory::from_events(vec![
        tools_available_event(conversation_id, 0, vec![tool_definition("first")]),
        tools_available_event(conversation_id, 1, Vec::new()),
    ])
    .expect("the conversation should be valid");

    assert!(
        ConversationView::new(&conversation)
            .available_tools()
            .is_empty()
    );
}

#[test]
fn a_conversation_without_a_tools_available_declaration_has_no_tools() {
    let conversation_id = ConversationId::new();

    let conversation = ConversationHistory::from_events(vec![user_event(conversation_id, 0)])
        .expect("the conversation should be valid");

    assert!(
        ConversationView::new(&conversation)
            .available_tools()
            .is_empty()
    );
}

#[test]
fn conversation_rejects_invalid_deserialized_tool_definitions() {
    let conversation_id = ConversationId::new();
    let conversation_event: ConversationEventRecord = serde_json::from_value(json!({
        "conversation_id": conversation_id,
        "position": 0,
        "id": ConversationEventId::new(),
        "timestamp": "2026-08-22T18:42:31.482Z",
        "schema_version": 13,
        "type": "context",
        "kind": "tools_available",
        "tools": [{
            "name": "shell",
            "description": "   ",
            "parameters": { "type": "object" },
            "result": { "type": "object" }
        }]
    }))
    .expect("derived deserialization should construct the conversation event");

    assert_eq!(
        ConversationHistory::from_events(vec![conversation_event]),
        Err(InvalidConversation::InvalidEvent {
            position: 0,
            reason: "tool description must not be empty".to_owned(),
        })
    );
}

#[test]
fn conversation_rejects_an_empty_user_event() {
    let conversation_id = ConversationId::new();
    let conversation_event: ConversationEventRecord = serde_json::from_value(json!({
        "conversation_id": conversation_id,
        "position": 0,
        "id": ConversationEventId::new(),
        "timestamp": "2026-08-22T18:42:31.482Z",
        "schema_version": 13,
        "type": "user",
        "content": []
    }))
    .expect("derived deserialization should construct the conversation event");

    assert_eq!(
        ConversationHistory::from_events(vec![conversation_event]),
        Err(InvalidConversation::InvalidEvent {
            position: 0,
            reason: "user content must not be empty".to_owned(),
        })
    );
}

#[test]
fn conversation_rejects_a_model_event_without_its_model_request() {
    let conversation_id = ConversationId::new();
    let assistant = record(
        conversation_id,
        0,
        ConversationEventId::new(),
        ConversationEvent::AssistantResponse(
            AssistantResponse::new(ConversationEventId::new(), "Hello.".to_owned())
                .expect("the assistant response should be valid"),
        ),
    );

    let error = ConversationHistory::from_events(vec![assistant])
        .expect_err("a model event without its request should be rejected");
    assert!(matches!(
        error,
        InvalidConversation::InvalidReference { position: 0, .. }
    ));
}

#[test]
fn conversation_accepts_references_within_a_turn() {
    let conversation_id = ConversationId::new();
    let user_id = ConversationEventId::new();
    let turn_id = ConversationEventId::new();
    let request_id = ConversationEventId::new();
    let tool_request_id = ConversationEventId::new();
    let events = vec![
        record(
            conversation_id,
            0,
            user_id,
            ConversationEvent::User(
                User::new(vec![UserContent::Text("Hello".to_owned())])
                    .expect("the user event should be valid"),
            ),
        ),
        record(
            conversation_id,
            1,
            turn_id,
            ConversationEvent::TurnStart(TurnStart::new(Some(user_id), 0)),
        ),
        record(
            conversation_id,
            2,
            request_id,
            ConversationEvent::ModelRequest(
                ModelRequest::new(turn_id, source(), 1, Vec::new(), None, None)
                    .expect("the model request should be valid"),
            ),
        ),
        record(
            conversation_id,
            3,
            tool_request_id,
            ConversationEvent::ToolRequest(
                ToolRequest::try_new(
                    request_id,
                    ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
                    json!({ "command": "pwd" }),
                    None,
                )
                .expect("the tool request should be valid"),
            ),
        ),
        record(
            conversation_id,
            4,
            ConversationEventId::new(),
            ConversationEvent::ToolResponse(ToolResponse::new(
                tool_request_id,
                ToolOutcome::succeeded(json!({ "stdout": "/tmp" })),
            )),
        ),
        record(
            conversation_id,
            5,
            ConversationEventId::new(),
            ConversationEvent::ModelResponse(
                ModelResponse::new(
                    request_id,
                    vec![tool_request_id],
                    ModelOutcome::Succeeded,
                    None,
                )
                .expect("the model response should be valid"),
            ),
        ),
        record(
            conversation_id,
            6,
            ConversationEventId::new(),
            ConversationEvent::TurnEnd(
                TurnEnd::new(turn_id, TurnOutcome::Succeeded)
                    .expect("the turn end should be valid"),
            ),
        ),
    ];

    let conversation = ConversationHistory::from_events(events)
        .expect("a complete turn reference chain should be valid");
    assert_eq!(conversation.events().len(), 7);
}

#[test]
fn conversation_rejects_a_missing_turn_start() {
    let conversation_id = ConversationId::new();
    let request_id = ConversationEventId::new();
    let events = vec![
        user_event(conversation_id, 0),
        record(
            conversation_id,
            1,
            request_id,
            ConversationEvent::ModelRequest(
                ModelRequest::new(
                    ConversationEventId::new(),
                    source(),
                    0,
                    Vec::new(),
                    None,
                    None,
                )
                .expect("the model request should be valid"),
            ),
        ),
    ];

    let error = ConversationHistory::from_events(events)
        .expect_err("a model request without its turn should be rejected");
    assert!(matches!(
        error,
        InvalidConversation::InvalidReference { position: 1, .. }
    ));
}

#[test]
fn conversation_rejects_a_wrong_tool_response_target() {
    let conversation_id = ConversationId::new();
    let turn_id = ConversationEventId::new();
    let request_id = ConversationEventId::new();
    let events = vec![
        record(
            conversation_id,
            0,
            turn_id,
            ConversationEvent::TurnStart(TurnStart::new(None, 0)),
        ),
        record(
            conversation_id,
            1,
            request_id,
            ConversationEvent::ModelRequest(
                ModelRequest::new(turn_id, source(), 0, Vec::new(), None, None)
                    .expect("the model request should be valid"),
            ),
        ),
        record(
            conversation_id,
            2,
            ConversationEventId::new(),
            ConversationEvent::ToolResponse(ToolResponse::new(
                request_id,
                ToolOutcome::succeeded(json!({ "ok": true })),
            )),
        ),
    ];

    let error = ConversationHistory::from_events(events)
        .expect_err("a tool response must reference a tool request");
    assert!(matches!(
        error,
        InvalidConversation::InvalidReference { position: 2, .. }
    ));
}

#[test]
fn conversation_rejects_two_terminal_model_responses() {
    let conversation_id = ConversationId::new();
    let turn_id = ConversationEventId::new();
    let request_id = ConversationEventId::new();
    let events = vec![
        record(
            conversation_id,
            0,
            turn_id,
            ConversationEvent::TurnStart(TurnStart::new(None, 0)),
        ),
        record(
            conversation_id,
            1,
            request_id,
            ConversationEvent::ModelRequest(
                ModelRequest::new(turn_id, source(), 0, Vec::new(), None, None)
                    .expect("the model request should be valid"),
            ),
        ),
        record(
            conversation_id,
            2,
            ConversationEventId::new(),
            ConversationEvent::ModelResponse(
                ModelResponse::new(request_id, Vec::new(), ModelOutcome::Succeeded, None)
                    .expect("the model response should be valid"),
            ),
        ),
        record(
            conversation_id,
            3,
            ConversationEventId::new(),
            ConversationEvent::ModelResponse(
                ModelResponse::new(request_id, Vec::new(), ModelOutcome::Succeeded, None)
                    .expect("the second model response should be valid"),
            ),
        ),
    ];

    let error = ConversationHistory::from_events(events)
        .expect_err("a model request must have one terminal response");
    assert!(matches!(
        error,
        InvalidConversation::InvalidReference { position: 3, .. }
    ));
}

#[test]
fn conversation_rejects_overlapping_model_response_outputs() {
    let conversation_id = ConversationId::new();
    let turn_id = ConversationEventId::new();
    let first_request_id = ConversationEventId::new();
    let second_request_id = ConversationEventId::new();
    let assistant_id = ConversationEventId::new();
    let events = vec![
        record(
            conversation_id,
            0,
            turn_id,
            ConversationEvent::TurnStart(TurnStart::new(None, 0)),
        ),
        record(
            conversation_id,
            1,
            first_request_id,
            ConversationEvent::ModelRequest(
                ModelRequest::new(turn_id, source(), 0, Vec::new(), None, None)
                    .expect("the first model request should be valid"),
            ),
        ),
        record(
            conversation_id,
            2,
            assistant_id,
            ConversationEvent::AssistantResponse(
                AssistantResponse::new(first_request_id, "First.".to_owned())
                    .expect("the assistant response should be valid"),
            ),
        ),
        record(
            conversation_id,
            3,
            second_request_id,
            ConversationEvent::ModelRequest(
                ModelRequest::new(turn_id, source(), 2, Vec::new(), None, None)
                    .expect("the second model request should be valid"),
            ),
        ),
        record(
            conversation_id,
            4,
            ConversationEventId::new(),
            ConversationEvent::ModelResponse(
                ModelResponse::new(
                    second_request_id,
                    vec![assistant_id],
                    ModelOutcome::Succeeded,
                    None,
                )
                .expect("the model response should be valid"),
            ),
        ),
    ];

    let error = ConversationHistory::from_events(events)
        .expect_err("a model response output must belong to the same request");
    assert!(matches!(
        error,
        InvalidConversation::InvalidReference { position: 4, .. }
    ));
}

#[test]
fn conversation_rejects_a_model_specific_event_without_a_request() {
    let conversation_id = ConversationId::new();
    let event = ConversationEvent::ModelSpecificEvent(
        ModelSpecificEvent::new(
            ConversationEventId::new(),
            "reasoning".to_owned(),
            1,
            json!({}),
            Some("Thinking.".to_owned()),
        )
        .expect("the model specific event should be valid"),
    );

    let error = ConversationHistory::from_events(vec![record(
        conversation_id,
        0,
        ConversationEventId::new(),
        event,
    )])
    .expect_err("a model specific event without its request should be rejected");
    assert!(matches!(
        error,
        InvalidConversation::InvalidReference { position: 0, .. }
    ));
}

#[test]
fn a_model_specific_event_round_trips_its_provider_payload() {
    let provider_event_type = "reasoning".to_owned();
    let message = "Thinking.".to_owned();
    let event = ModelSpecificEvent::new(
        ConversationEventId::new(),
        provider_event_type,
        2,
        json!({ "native": "kept" }),
        Some(message),
    )
    .expect("the model specific event should be valid");

    let restored: ModelSpecificEvent =
        serde_json::from_value(serde_json::to_value(&event).expect("the event should serialize"))
            .expect("the event should deserialize");
    assert_eq!(restored, event);
}

#[test]
fn an_operation_failure_round_trips_category_message_and_details() {
    let failure = OperationFailure::try_new(
        FailureCategory::TimedOut,
        "the shell command timed out after 1 seconds".to_owned(),
        Some(json!({ "timeout_seconds": 1 })),
    )
    .expect("the failure should be valid");

    let restored: OperationFailure = serde_json::from_value(
        serde_json::to_value(&failure).expect("the failure should serialize"),
    )
    .expect("the failure should deserialize");
    assert_eq!(restored, failure);
    assert_eq!(restored.category(), FailureCategory::TimedOut);
    assert_eq!(
        restored.message(),
        "the shell command timed out after 1 seconds"
    );
    assert_eq!(restored.details(), Some(&json!({ "timeout_seconds": 1 })));
    assert!(restored.retryable());
    assert!(!restored.execution_known());
}
