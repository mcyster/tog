use schemars::json_schema;
use serde_json::json;
use time::OffsetDateTime;

use super::history::InvalidConversation;
use super::{Conversation, ConversationHistory, ConversationId};
use crate::conversation_event::{
    ConversationCommandId, ConversationEventId, ConversationEventKind, ConversationEventRecord,
    ConversationFact, ConversationMessage, ConversationTurnId, ModelInvocationId,
    StoredConversationEventKind, ToolDefinition, ToolName, UserContent,
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

fn tools_available_event(
    conversation_id: ConversationId,
    position: u64,
    tools: Vec<ToolDefinition>,
) -> ConversationEventRecord {
    ConversationEventRecord {
        conversation_id,
        position,
        id: ConversationEventId::new(),
        timestamp: OffsetDateTime::UNIX_EPOCH,
        schema_version: 13,
        kind: StoredConversationEventKind::Shared(ConversationEventKind::Fact(
            ConversationFact::ToolsAvailable { tools },
        )),
    }
}

fn user_event(conversation_id: ConversationId, position: u64) -> ConversationEventRecord {
    ConversationEventRecord {
        conversation_id,
        position,
        id: ConversationEventId::new(),
        timestamp: OffsetDateTime::UNIX_EPOCH,
        schema_version: 7,
        kind: StoredConversationEventKind::Shared(ConversationEventKind::Fact(
            ConversationFact::Message {
                message: ConversationMessage::User {
                    caused_by: Some(ConversationCommandId::new()),
                    content: vec![UserContent::Text(format!("event {position}"))],
                },
                turn_id: None,
            },
        )),
    }
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
fn conversation_accepts_gaps_left_by_command_records() {
    let conversation_id = ConversationId::new();

    let conversation = ConversationHistory::from_events(vec![
        user_event(conversation_id, 1),
        user_event(conversation_id, 3),
    ])
    .expect("the conversation should allow command positions between events");

    assert_eq!(conversation.events()[1].position, 3);
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

    let available_tools = conversation.available_tools();
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

    assert!(conversation.available_tools().is_empty());
}

#[test]
fn a_conversation_without_a_tools_available_declaration_has_no_tools() {
    let conversation_id = ConversationId::new();

    let conversation = ConversationHistory::from_events(vec![user_event(conversation_id, 0)])
        .expect("the conversation should be valid");

    assert!(conversation.available_tools().is_empty());
}

#[test]
fn conversation_rejects_invalid_deserialized_tool_definitions() {
    let conversation_id = ConversationId::new();
    let event_id = ConversationEventId::new();
    let conversation_event: ConversationEventRecord = serde_json::from_value(json!({
        "conversation_id": conversation_id,
        "position": 0,
        "id": event_id,
        "timestamp": "2026-08-22T18:42:31.482Z",
        "schema_version": 13,
        "class": "fact",
        "event": {
            "tools": [{
                "name": "shell",
                "description": "   ",
                "parameters": { "type": "object" },
                "result": { "type": "object" }
            }]
        }
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
fn conversation_rejects_invalid_deserialized_model_events() {
    let conversation_id = ConversationId::new();
    let event_id = ConversationEventId::new();
    let conversation_event: ConversationEventRecord = serde_json::from_value(json!({
        "conversation_id": conversation_id,
        "position": 0,
        "id": event_id,
        "timestamp": "2026-08-22T18:42:31.482Z",
        "schema_version": 11,
        "class": "fact",
        "event": {
            "type": "assistant",
            "turn_id": ConversationTurnId::new(),
            "invocation_id": ModelInvocationId::new(),
            "response": { "message": "   " }
        }
    }))
    .expect("derived deserialization should construct the conversation event");

    assert_eq!(
        ConversationHistory::from_events(vec![conversation_event]),
        Err(InvalidConversation::InvalidEvent {
            position: 0,
            reason: "assistant response message must not be empty".to_owned(),
        })
    );
}

#[test]
fn conversation_rejects_invalid_deserialized_model_communications() {
    let conversation_id = ConversationId::new();
    let event_id = ConversationEventId::new();
    let conversation_event: ConversationEventRecord = serde_json::from_value(json!({
        "conversation_id": conversation_id,
        "position": 0,
        "id": event_id,
        "timestamp": "2026-08-22T18:42:31.482Z",
        "schema_version": 11,
        "class": "fact",
        "event": {
            "type": "communication",
            "turn_id": ConversationTurnId::new(),
            "invocation_id": ModelInvocationId::new(),
            "communication": {
                "message": "reasoning",
                "importance": "detailed",
                "subtype": "   "
            }
        }
    }))
    .expect("derived deserialization should construct the conversation event");

    assert_eq!(
        ConversationHistory::from_events(vec![conversation_event]),
        Err(InvalidConversation::InvalidEvent {
            position: 0,
            reason: "model communication subtype must not be empty".to_owned(),
        })
    );
}

#[test]
fn conversation_rejects_invalid_deserialized_model_problems() {
    let conversation_id = ConversationId::new();
    let event_id = ConversationEventId::new();
    let conversation_event: ConversationEventRecord = serde_json::from_value(json!({
        "conversation_id": conversation_id,
        "position": 0,
        "id": event_id,
        "timestamp": "2026-08-22T18:42:31.482Z",
        "schema_version": 11,
        "class": "fact",
        "event": {
            "type": "problem",
            "turn_id": null,
            "invocation_id": null,
            "problem": {
                "category": "issue",
                "detail": {
                    "type": "refusal",
                    "message": "   "
                }
            }
        }
    }))
    .expect("derived deserialization should construct the conversation event");

    assert_eq!(
        ConversationHistory::from_events(vec![conversation_event]),
        Err(InvalidConversation::InvalidEvent {
            position: 0,
            reason: "conversation problem message must not be empty".to_owned(),
        })
    );
}

#[test]
fn conversation_rejects_empty_deserialized_model_data() {
    let conversation_id = ConversationId::new();
    let event_id = ConversationEventId::new();
    let conversation_event: ConversationEventRecord = serde_json::from_value(json!({
        "conversation_id": conversation_id,
        "position": 0,
        "id": event_id,
        "timestamp": "2026-08-22T18:42:31.482Z",
        "schema_version": 11,
        "class": "fact",
        "event": {
            "type": "assistant",
            "turn_id": ConversationTurnId::new(),
            "invocation_id": ModelInvocationId::new(),
            "data": {},
            "response": { "message": "The answer is 42." }
        }
    }))
    .expect("derived deserialization should construct the conversation event");

    assert_eq!(
        ConversationHistory::from_events(vec![conversation_event]),
        Err(InvalidConversation::InvalidEvent {
            position: 0,
            reason: "model data content must not be empty".to_owned(),
        })
    );
}

#[test]
fn conversation_loads_problem_events_from_earlier_schema_versions() {
    let conversation_id = ConversationId::new();
    let event_id = ConversationEventId::new();
    let conversation_event: ConversationEventRecord = serde_json::from_value(json!({
        "conversation_id": conversation_id,
        "position": 0,
        "id": event_id,
        "timestamp": "2026-08-22T18:42:31.482Z",
        "schema_version": 11,
        "class": "fact",
        "event": {
            "type": "problem",
            "problem": {
                "category": "invocation",
                "detail": {
                    "type": "transport",
                    "message": "The model provider could not be reached."
                }
            }
        }
    }))
    .expect("the earlier problem event should deserialize");

    let conversation = ConversationHistory::from_events(vec![conversation_event])
        .expect("the conversation should be valid");

    assert!(matches!(
        conversation.events()[0].kind,
        StoredConversationEventKind::Shared(ConversationEventKind::Fact(
            ConversationFact::Message {
                message: ConversationMessage::Problem { .. },
                ..
            }
        ))
    ));
}
