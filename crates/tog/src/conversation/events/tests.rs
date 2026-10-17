use std::str::FromStr;

use serde_json::json;

use super::{
    AssistantResponse, Automation, Context, ConversationEventId, FailureCategory,
    InvalidModelSpecificEvent, InvalidUser, ModelContinuation, ModelData, ModelId, ModelOutcome,
    ModelRequest, ModelResponse, ModelSource, ModelSpecificEvent, OperationFailure, ProviderId,
    Tool, ToolAvailability, ToolOutcome, ToolRequest, ToolResponse, Tools, TurnEnd, TurnOutcome,
    TurnStart, User, UserContent,
};
use crate::conversation::{ConversationEvent, ConversationEventPayload, ConversationId};
use crate::conversation_event_store::ConversationEventRecord;

fn record(
    conversation_id: ConversationId,
    position: u64,
    payload: ConversationEventPayload,
) -> ConversationEventRecord {
    ConversationEventRecord::new(position, ConversationEvent::new(conversation_id, payload))
}

#[test]
fn conversation_event_identifier_round_trips_through_its_display_and_serialized_forms() {
    let event_id = ConversationEventId::new();

    let serialized = serde_json::to_value(event_id).expect("the event identifier should serialize");
    assert_eq!(serialized, serde_json::Value::String(event_id.to_string()));
    let deserialized: ConversationEventId =
        serde_json::from_value(serialized).expect("the event identifier should parse");
    assert_eq!(deserialized, event_id);
}

#[test]
fn conversation_event_identifier_accepts_an_unprefixed_uuid() {
    let uuid_text = uuid::Uuid::now_v7().simple().to_string();

    let reparsed = ConversationEventId::from_str(&uuid_text)
        .expect("an unprefixed uuid should parse as an event identifier");

    assert_eq!(reparsed.to_string(), format!("evt_{uuid_text}"));
}

fn source() -> ModelSource {
    ModelSource::new(
        ProviderId::from_str("test").expect("the provider should be valid"),
        ModelId::from_str("test-model").expect("the model should be valid"),
    )
}

fn tool_definition(name: &str) -> crate::conversation::ToolDefinition {
    crate::conversation::ToolDefinition::try_new(
        crate::conversation::ToolName::try_new(name.to_owned())
            .expect("the tool name should be valid"),
        "Run a command.".to_owned(),
        schemars::json_schema!({ "type": "object" }),
        schemars::json_schema!({ "type": "object" }),
    )
    .expect("the tool definition should be valid")
}

#[test]
fn event_records_serialize_with_a_flat_type_discriminator() {
    let conversation_id = ConversationId::new();
    let user = record(
        conversation_id,
        0,
        ConversationEventPayload::User(
            User::new(vec![UserContent::Text("Hello".to_owned())])
                .expect("the user event should be valid"),
        ),
    );
    let value = serde_json::to_value(&user).expect("the record should serialize");
    assert_eq!(value["type"], "user");
    assert_eq!(
        value["content"][0],
        json!({ "type": "text", "value": "Hello" })
    );
    assert_eq!(
        value["conversation_id"],
        serde_json::to_value(conversation_id).expect("the id should serialize")
    );
    assert!(value["id"].is_string());
    assert!(value["timestamp"].is_string());
    assert!(value.get("class").is_none());
    assert!(value.get("kind").is_none());
    assert!(value.get("event").is_none());
    assert!(value.get("payload").is_none());
    let restored: ConversationEventRecord =
        serde_json::from_value(value).expect("the record should deserialize");
    assert_eq!(restored, user);
}

#[test]
fn every_event_kind_keeps_its_stable_discriminator() {
    let conversation_id = ConversationId::new();
    let id = ConversationEventId::new();
    let tools = Tools::new(vec![Tool::new(
        tool_definition("shell"),
        ToolAvailability::Immediate,
    )]);
    let events = [
        (
            ConversationEventPayload::TurnStart(TurnStart::new(None, 0)),
            "turn_start",
        ),
        (
            ConversationEventPayload::TurnEnd(
                TurnEnd::new(id, TurnOutcome::Succeeded).expect("the turn end should be valid"),
            ),
            "turn_end",
        ),
        (
            ConversationEventPayload::ModelRequest(
                ModelRequest::new(id, source(), 0, Vec::new(), None, None)
                    .expect("the model request should be valid"),
            ),
            "model_request",
        ),
        (
            ConversationEventPayload::ModelResponse(
                ModelResponse::new(
                    id,
                    Vec::new(),
                    ModelOutcome::Succeeded,
                    ModelContinuation::Complete,
                    None,
                )
                .expect("the model response should be valid"),
            ),
            "model_response",
        ),
        (
            ConversationEventPayload::AssistantResponse(
                AssistantResponse::new(id, "Hello.".to_owned())
                    .expect("the assistant response should be valid"),
            ),
            "assistant_response",
        ),
        (
            ConversationEventPayload::ToolRequest(
                ToolRequest::try_new(
                    id,
                    crate::conversation::ToolName::from_str("shell")
                        .expect("the tool name should be valid"),
                    json!({ "command": "pwd" }),
                    None,
                )
                .expect("the tool request should be valid"),
            ),
            "tool_request",
        ),
        (
            ConversationEventPayload::ToolResponse(ToolResponse::new(
                id,
                ToolOutcome::succeeded(json!({ "ok": true })),
            )),
            "tool_response",
        ),
        (
            ConversationEventPayload::ModelSpecificEvent(
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
            ConversationEventPayload::Automation(
                Automation::new("auto".to_owned()).expect("the automation should be valid"),
            ),
            "automation",
        ),
        (
            ConversationEventPayload::Context(
                Context::try_new(
                    "workspace".to_owned(),
                    "tog.workspace".to_owned(),
                    json!({}),
                )
                .expect("the context should be valid"),
            ),
            "context",
        ),
        (ConversationEventPayload::Tools(tools), "tools"),
    ];

    for (payload, expected_type) in events {
        let value = serde_json::to_value(record(conversation_id, 0, payload))
            .expect("the record should serialize");
        assert_eq!(
            value["type"], expected_type,
            "the {expected_type} event should keep its discriminator"
        );
    }
}

#[test]
fn a_complete_event_keeps_its_identity_across_serialization() {
    let conversation_id = ConversationId::new();
    let event = ConversationEvent::new(
        conversation_id,
        ConversationEventPayload::Context(
            Context::try_new(
                "workspace".to_owned(),
                "tog.workspace".to_owned(),
                json!({}),
            )
            .expect("the context should be valid"),
        ),
    );

    let value = serde_json::to_value(&event).expect("the event should serialize");
    let restored: ConversationEvent =
        serde_json::from_value(value).expect("the event should deserialize");
    assert_eq!(restored.id(), event.id());
    assert_eq!(restored.conversation_id(), conversation_id);
    assert_eq!(restored.timestamp(), event.timestamp());
    assert_eq!(restored.payload(), event.payload());
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
fn a_tools_event_persists_full_definitions_with_availability() {
    let conversation_id = ConversationId::new();
    let tools = Tools::new(vec![
        Tool::new(tool_definition("shell"), ToolAvailability::Immediate),
        Tool::new(tool_definition("search"), ToolAvailability::Discoverable),
    ]);
    let event = record(
        conversation_id,
        0,
        ConversationEventPayload::Tools(tools.clone()),
    );

    let value = serde_json::to_value(&event).expect("the tools event should serialize");
    assert_eq!(value["type"], "tools");
    assert_eq!(value["tools"][0]["availability"], "immediate");
    assert_eq!(value["tools"][1]["availability"], "discoverable");
    assert_eq!(value["tools"][0]["definition"]["name"], "shell");
    let restored: ConversationEventRecord =
        serde_json::from_value(value).expect("the tools event should deserialize");
    assert_eq!(restored, event);
    assert!(matches!(
        restored.event().payload(),
        ConversationEventPayload::Tools(restored_tools) if restored_tools == &tools
    ));
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
fn a_model_specific_event_accepts_a_whitespace_only_message() {
    let event = ModelSpecificEvent::new(
        ConversationEventId::new(),
        "content_delta".to_owned(),
        1,
        json!({}),
        Some(" ".to_owned()),
    )
    .expect("a whitespace-only message should be valid");

    assert_eq!(event.message(), Some(" "));
}

#[test]
fn a_model_specific_event_rejects_an_empty_message() {
    let error = ModelSpecificEvent::new(
        ConversationEventId::new(),
        "content_delta".to_owned(),
        1,
        json!({}),
        Some(String::new()),
    )
    .expect_err("an empty message should be rejected");

    assert_eq!(error, InvalidModelSpecificEvent::EmptyMessage);
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
fn event_payloads_describe_themselves_concisely() {
    let id = ConversationEventId::new();
    let tools = Tools::new(vec![Tool::new(
        tool_definition("shell"),
        ToolAvailability::Immediate,
    )]);
    let cases = [
        (
            ConversationEventPayload::User(
                User::new(vec![UserContent::Text("test 123".to_owned())])
                    .expect("the user event should be valid"),
            ),
            "user test 123",
        ),
        (
            ConversationEventPayload::TurnStart(TurnStart::new(None, 3)),
            "turn_start input through 3",
        ),
        (
            ConversationEventPayload::TurnEnd(
                TurnEnd::new(id, TurnOutcome::Succeeded).expect("the turn end should be valid"),
            ),
            "turn_end succeeded",
        ),
        (
            ConversationEventPayload::AssistantResponse(
                AssistantResponse::new(id, "Hello, world.".to_owned())
                    .expect("the assistant response should be valid"),
            ),
            "assistant_response Hello, world.",
        ),
        (
            ConversationEventPayload::ToolRequest(
                ToolRequest::try_new(
                    id,
                    crate::conversation::ToolName::from_str("shell")
                        .expect("the tool name should be valid"),
                    json!({ "command": "echo \"test 123\"" }),
                    None,
                )
                .expect("the tool request should be valid"),
            ),
            r#"tool_request shell({"command":"echo \"test 123\""})"#,
        ),
        (
            ConversationEventPayload::ToolResponse(ToolResponse::new(
                id,
                ToolOutcome::succeeded(json!({ "ok": true })),
            )),
            r#"tool_response {"ok":true}"#,
        ),
        (
            ConversationEventPayload::ModelRequest(
                ModelRequest::new(id, source(), 0, Vec::new(), None, None)
                    .expect("the model request should be valid"),
            ),
            "model_request test/test-model input through 0",
        ),
        (
            ConversationEventPayload::ModelResponse(
                ModelResponse::new(
                    id,
                    Vec::new(),
                    ModelOutcome::Succeeded,
                    ModelContinuation::Complete,
                    None,
                )
                .expect("the model response should be valid"),
            ),
            "model_response succeeded complete with 0 outputs",
        ),
        (
            ConversationEventPayload::ModelSpecificEvent(
                ModelSpecificEvent::new(
                    id,
                    "reasoning".to_owned(),
                    1,
                    json!({}),
                    Some("Thinking.".to_owned()),
                )
                .expect("the model specific event should be valid"),
            ),
            "model_specific_event reasoning: Thinking.",
        ),
        (
            ConversationEventPayload::Automation(
                Automation::new("auto".to_owned()).expect("the automation should be valid"),
            ),
            "automation auto",
        ),
        (
            ConversationEventPayload::Context(
                Context::try_new(
                    "workspace".to_owned(),
                    "tog.workspace".to_owned(),
                    json!({}),
                )
                .expect("the context should be valid"),
            ),
            "context workspace (tog.workspace)",
        ),
        (ConversationEventPayload::Tools(tools), "tools shell"),
    ];

    for (payload, expected) in cases {
        assert_eq!(payload.describe(), expected);
    }
}

#[test]
fn failed_event_payloads_describe_their_failure() {
    let id = ConversationEventId::new();
    let failure = OperationFailure::try_new(
        FailureCategory::TimedOut,
        "the shell command timed out".to_owned(),
        None,
    )
    .expect("the failure should be valid");

    let cases: [(&str, String); 3] = [
        (
            "turn_end",
            ConversationEventPayload::TurnEnd(
                TurnEnd::new(
                    id,
                    TurnOutcome::Failed {
                        failure: failure.clone(),
                    },
                )
                .expect("the turn end should be valid"),
            )
            .describe(),
        ),
        (
            "model_response",
            ConversationEventPayload::ModelResponse(
                ModelResponse::new(
                    id,
                    Vec::new(),
                    ModelOutcome::Failed {
                        failure: failure.clone(),
                    },
                    ModelContinuation::Complete,
                    None,
                )
                .expect("the model response should be valid"),
            )
            .describe(),
        ),
        (
            "tool_response",
            ConversationEventPayload::ToolResponse(ToolResponse::new(
                id,
                ToolOutcome::Failed {
                    failure: failure.clone(),
                },
            ))
            .describe(),
        ),
    ];

    for (kind, description) in cases {
        assert_eq!(
            description,
            format!("{kind} failed: the shell command timed out")
        );
    }
}

#[test]
fn a_model_response_round_trips_its_outcome_and_continuation_every_time() {
    let id = ConversationEventId::new();
    let completed = ModelResponse::new(
        id,
        Vec::new(),
        ModelOutcome::Succeeded,
        ModelContinuation::Complete,
        None,
    )
    .expect("the completed response should be valid");
    let continuing = ModelResponse::new(
        id,
        Vec::new(),
        ModelOutcome::Succeeded,
        ModelContinuation::Continue,
        None,
    )
    .expect("the continuing response should be valid");
    let failed = ModelResponse::new(
        id,
        Vec::new(),
        ModelOutcome::Failed {
            failure: OperationFailure::try_new(
                FailureCategory::ProviderFailure,
                "the model failed".to_owned(),
                None,
            )
            .expect("the failure should be valid"),
        },
        ModelContinuation::Complete,
        None,
    )
    .expect("the failed response should be valid");

    let completed_value = serde_json::to_value(&completed).expect("the response should serialize");
    assert_eq!(completed_value["continuation"]["type"], "complete");
    let restored: ModelResponse =
        serde_json::from_value(completed_value).expect("the completed response should deserialize");
    assert_eq!(restored, completed);
    assert_eq!(restored.continuation(), ModelContinuation::Complete);

    let continuing_value =
        serde_json::to_value(&continuing).expect("the response should serialize");
    assert_eq!(continuing_value["continuation"]["type"], "continue");
    let restored: ModelResponse = serde_json::from_value(continuing_value)
        .expect("the continuing response should deserialize");
    assert_eq!(restored.continuation(), ModelContinuation::Continue);

    let failed_value = serde_json::to_value(&failed).expect("the response should serialize");
    assert_eq!(
        failed_value["continuation"]["type"], "complete",
        "a failed response still records its continuation"
    );
    let restored: ModelResponse =
        serde_json::from_value(failed_value).expect("the failed response should deserialize");
    assert_eq!(restored, failed);
    assert_eq!(restored.continuation(), ModelContinuation::Complete);

    assert_eq!(completed.describe(), "succeeded complete with 0 outputs");
    assert_eq!(continuing.describe(), "succeeded continue with 0 outputs");
    assert_eq!(failed.describe(), "failed: the model failed");
}

#[test]
fn a_failed_model_response_may_also_request_continuation() {
    let id = ConversationEventId::new();
    let failed_continuing = ModelResponse::new(
        id,
        Vec::new(),
        ModelOutcome::Failed {
            failure: OperationFailure::try_new(
                FailureCategory::RateLimited,
                "the model rate-limited the attempt".to_owned(),
                None,
            )
            .expect("the failure should be valid"),
        },
        ModelContinuation::Continue,
        None,
    )
    .expect("a failed response may request continuation");

    assert_eq!(
        failed_continuing.continuation(),
        ModelContinuation::Continue
    );
    assert_eq!(
        failed_continuing.describe(),
        "failed: the model rate-limited the attempt"
    );
}

#[test]
fn multiline_text_payloads_describe_as_one_line() {
    let id = ConversationEventId::new();
    let user = ConversationEventPayload::User(
        User::new(vec![UserContent::Text("line one\n\nline two".to_owned())])
            .expect("the user event should be valid"),
    );
    let assistant = ConversationEventPayload::AssistantResponse(
        AssistantResponse::new(id, "first line\nsecond line".to_owned())
            .expect("the assistant response should be valid"),
    );

    assert_eq!(user.describe(), "user line one line two");
    assert_eq!(
        assistant.describe(),
        "assistant_response first line second line"
    );
}

#[test]
fn a_tool_request_keeps_its_provider_call_identifier_as_opaque_data() {
    let request = ToolRequest::try_new(
        ConversationEventId::new(),
        crate::conversation::ToolName::from_str("shell").expect("the tool name should be valid"),
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
