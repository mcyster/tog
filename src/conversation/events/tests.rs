use std::str::FromStr;

use serde_json::json;
use time::OffsetDateTime;

use super::{
    AssistantResponse, Automation, Context, ConversationEvent, ConversationEventId,
    ConversationEventRecord, FailureCategory, InvalidUser, ModelData, ModelId, ModelOutcome,
    ModelRequest, ModelResponse, ModelSource, ModelSpecificEvent, OperationFailure, ProviderId,
    ToolOutcome, ToolRequest, ToolResponse, TurnEnd, TurnOutcome, TurnStart, User, UserContent,
};
use crate::conversation::ConversationId;
use crate::toolset::{ToolAvailability, ToolDefinition, ToolName, Toolset, ToolsetEntry};

fn record(event: ConversationEvent) -> ConversationEventRecord {
    ConversationEventRecord {
        conversation_id: ConversationId::new(),
        position: 0,
        id: ConversationEventId::new(),
        timestamp: OffsetDateTime::UNIX_EPOCH,
        schema_version: 13,
        event,
    }
}

fn source() -> ModelSource {
    ModelSource::new(
        ProviderId::from_str("test").expect("the provider should be valid"),
        ModelId::from_str("test-model").expect("the model should be valid"),
    )
}

fn tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition::try_new(
        ToolName::try_new(name.to_owned()).expect("the tool name should be valid"),
        "Run a command.".to_owned(),
        schemars::json_schema!({ "type": "object" }),
        schemars::json_schema!({ "type": "object" }),
    )
    .expect("the tool definition should be valid")
}

#[test]
fn event_records_serialize_with_a_flat_type_discriminator() {
    let user = record(ConversationEvent::User(
        User::new(vec![UserContent::Text("Hello".to_owned())])
            .expect("the user event should be valid"),
    ));
    let value = serde_json::to_value(&user).expect("the record should serialize");
    assert_eq!(value["type"], "user");
    assert_eq!(
        value["content"][0],
        json!({ "type": "text", "value": "Hello" })
    );
    assert!(value.get("class").is_none());
    assert!(value.get("kind").is_none());
    assert!(value.get("event").is_none());
    let restored: ConversationEventRecord =
        serde_json::from_value(value).expect("the record should deserialize");
    assert_eq!(restored, user);
}

#[test]
fn every_event_kind_keeps_its_stable_discriminator() {
    let id = ConversationEventId::new();
    let toolset =
        Toolset::immediate(vec![tool_definition("shell")]).expect("the toolset should be valid");
    let events = [
        (
            ConversationEvent::TurnStart(TurnStart::new(None, 0)),
            "turn_start",
        ),
        (
            ConversationEvent::TurnEnd(
                TurnEnd::new(id, TurnOutcome::Succeeded).expect("the turn end should be valid"),
            ),
            "turn_end",
        ),
        (
            ConversationEvent::ModelRequest(
                ModelRequest::new(id, source(), 0, Vec::new(), None, None)
                    .expect("the model request should be valid"),
            ),
            "model_request",
        ),
        (
            ConversationEvent::ModelResponse(
                ModelResponse::new(id, Vec::new(), ModelOutcome::Succeeded, None)
                    .expect("the model response should be valid"),
            ),
            "model_response",
        ),
        (
            ConversationEvent::AssistantResponse(
                AssistantResponse::new(id, "Hello.".to_owned())
                    .expect("the assistant response should be valid"),
            ),
            "assistant_response",
        ),
        (
            ConversationEvent::ToolRequest(
                ToolRequest::try_new(
                    id,
                    ToolName::from_str("shell").expect("the tool name should be valid"),
                    json!({ "command": "pwd" }),
                    None,
                )
                .expect("the tool request should be valid"),
            ),
            "tool_request",
        ),
        (
            ConversationEvent::ToolResponse(ToolResponse::new(
                id,
                ToolOutcome::succeeded(json!({ "ok": true })),
            )),
            "tool_response",
        ),
        (
            ConversationEvent::ModelSpecificEvent(
                ModelSpecificEvent::new(
                    id,
                    "reasoning".to_owned(),
                    1,
                    json!({}),
                    Some("Thinking.".to_owned()),
                )
                .expect("the model specific event should be valid"),
            ),
            "model_specific_event",
        ),
        (
            ConversationEvent::Automation(
                Automation::new("auto".to_owned()).expect("the automation should be valid"),
            ),
            "automation",
        ),
        (
            ConversationEvent::Context(
                Context::try_new(
                    "workspace".to_owned(),
                    "tog.workspace".to_owned(),
                    json!({}),
                )
                .expect("the context should be valid"),
            ),
            "context",
        ),
        (ConversationEvent::Toolset(toolset), "toolset"),
    ];

    for (event, expected_type) in events {
        let value = serde_json::to_value(record(event)).expect("the record should serialize");
        assert_eq!(
            value["type"], expected_type,
            "the {expected_type} event should keep its discriminator"
        );
    }
}

#[test]
fn a_named_typed_context_round_trips_with_resolved_defaults() {
    let context = Context::try_new(
        String::new(),
        "tog.workspace".to_owned(),
        json!({ "note": "x" }),
    )
    .expect("the context should be valid");
    assert_eq!(
        context.name(),
        "tog.workspace",
        "the name defaults to the type"
    );
    assert_eq!(context.context_type(), "tog.workspace");

    let value = serde_json::to_value(&context).expect("the context should serialize");
    assert_eq!(value["name"], "tog.workspace");
    assert_eq!(value["context_type"], "tog.workspace");
    let restored: Context = serde_json::from_value(value).expect("the context should deserialize");
    assert_eq!(restored, context);

    let string_context = Context::try_new(String::new(), String::new(), json!("text"))
        .expect("the context should be valid");
    assert_eq!(string_context.context_type(), "string");
    assert_eq!(string_context.name(), "string");
    let value = serde_json::to_value(&string_context).expect("the string context should serialize");
    assert_eq!(value["context_type"], "string");
}

#[test]
fn context_explicit_names_distinguish_repeated_types() {
    let workspace_ctx = Context::try_new(
        "source".to_owned(),
        "extole.client".to_owned(),
        json!({ "client_id": "a" }),
    )
    .expect("the context should be valid");
    assert_eq!(workspace_ctx.name(), "source");
    assert_eq!(workspace_ctx.context_type(), "extole.client");
}

#[test]
fn a_toolset_persists_full_definitions_with_availability() {
    let toolset = Toolset::new(vec![
        ToolsetEntry::new(tool_definition("shell"), ToolAvailability::Immediate),
        ToolsetEntry::new(tool_definition("search"), ToolAvailability::Discoverable),
    ])
    .expect("the toolset should be valid");
    let event = record(ConversationEvent::Toolset(toolset.clone()));

    let value = serde_json::to_value(&event).expect("the toolset should serialize");
    assert_eq!(value["type"], "toolset");
    assert_eq!(value["entries"][0]["availability"], "immediate");
    assert_eq!(value["entries"][1]["availability"], "discoverable");
    assert_eq!(value["entries"][0]["definition"]["name"], "shell");
    let restored: ConversationEventRecord =
        serde_json::from_value(value).expect("the toolset should deserialize");
    assert_eq!(restored, event);
    assert_eq!(restored.event, ConversationEvent::Toolset(toolset));
}

#[test]
fn user_content_must_not_be_empty() {
    assert_eq!(User::new(Vec::new()), Err(InvalidUser::EmptyContent));
}

#[test]
fn a_model_specific_event_round_trips_its_payload_version_and_message() {
    let event = ModelSpecificEvent::new(
        ConversationEventId::new(),
        "reasoning".to_owned(),
        2,
        json!({ "native": "kept" }),
        Some("Thinking.".to_owned()),
    )
    .expect("the model specific event should be valid");

    let restored: ModelSpecificEvent =
        serde_json::from_value(serde_json::to_value(&event).expect("the event should serialize"))
            .expect("the event should deserialize");
    assert_eq!(restored, event);
    assert_eq!(restored.provider_payload_version(), 2);
    assert_eq!(restored.message(), Some("Thinking."));
}

#[test]
fn automation_round_trips_its_content() {
    let automation =
        Automation::new("an observed event".to_owned()).expect("the automation should be valid");
    let restored: Automation = serde_json::from_value(
        serde_json::to_value(&automation).expect("the event should serialize"),
    )
    .expect("the event should deserialize");
    assert_eq!(restored, automation);
    assert_eq!(restored.content(), "an observed event");
}

#[test]
fn an_operation_failure_round_trips_and_exposes_its_retry_guidance() {
    let failure = OperationFailure::try_new(
        FailureCategory::TimedOut,
        "the shell command timed out".to_owned(),
        Some(json!({ "timeout_seconds": 1 })),
    )
    .expect("the failure should be valid");

    let restored: OperationFailure = serde_json::from_value(
        serde_json::to_value(&failure).expect("the failure should serialize"),
    )
    .expect("the failure should deserialize");
    assert_eq!(restored, failure);
    assert_eq!(restored.category(), FailureCategory::TimedOut);
    assert!(restored.retryable());
    assert!(!restored.execution_known());
}

#[test]
fn a_tool_request_keeps_its_provider_call_identifier_as_opaque_data() {
    let request = ToolRequest::try_new(
        ConversationEventId::new(),
        ToolName::from_str("shell").expect("the tool name should be valid"),
        json!({ "command": "pwd" }),
        Some(
            ModelData::new(
                [("call_id".to_owned(), json!("call_native"))]
                    .into_iter()
                    .collect(),
            )
            .expect("the model data should be valid"),
        ),
    )
    .expect("the tool request should be valid");

    let restored: ToolRequest = serde_json::from_value(
        serde_json::to_value(&request).expect("the request should serialize"),
    )
    .expect("the request should deserialize");
    assert_eq!(restored, request);
    assert_eq!(
        restored
            .data()
            .expect("the data should be present")
            .content()["call_id"],
        "call_native"
    );
}
