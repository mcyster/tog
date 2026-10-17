use std::collections::HashMap;

use schemars::Schema;
use serde_json::{Value, json};

use super::{PROVIDER_PAYLOAD_VERSION, REASONING_ITEM_EVENT_TYPE};
use tog::conversation::{
    ConversationEventId, ConversationEventPayload, ModelSource, ModelSpecificEvent, ToolDefinition,
    ToolRequest, UserContent,
};
use tog::conversation_event_store::ConversationEventRecord;

pub(crate) fn responses_input(
    events: &[ConversationEventRecord],
    current_source: &ModelSource,
) -> Value {
    let provider_call_ids = provider_tool_call_ids(events);
    let model_request_sources = model_request_sources(events);
    let input = events
        .iter()
        .filter_map(|conversation_event| {
            let event = conversation_event.event();
            match event.payload() {
                ConversationEventPayload::User(user) => {
                    let text = user
                        .content()
                        .iter()
                        .map(|content| match content {
                            UserContent::Text(text) => text.as_str(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(json!({
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": text }],
                    }))
                }
                ConversationEventPayload::ModelSpecificEvent(specific_event) => {
                    replay_reasoning_item(specific_event, &model_request_sources, current_source)
                }
                ConversationEventPayload::AssistantResponse(response) => Some(json!({
                    "type": "message",
                    "role": "assistant",
                    "id": event.id().to_string(),
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": response.content(),
                        "annotations": [],
                    }],
                })),
                ConversationEventPayload::ToolRequest(request) => {
                    let call_id = provider_call_ids
                        .get(&event.id())
                        .cloned()
                        .unwrap_or_else(|| event.id().to_string());
                    Some(json!({
                        "type": "function_call",
                        "id": event.id().to_string(),
                        "call_id": call_id,
                        "name": request.tool_name().as_str(),
                        "arguments": serde_json::to_string(request.arguments())
                            .expect("the tool request arguments should serialize"),
                    }))
                }
                ConversationEventPayload::ToolResponse(response) => {
                    let call_id = provider_call_ids
                        .get(&response.tool_request_id())
                        .cloned()
                        .unwrap_or_else(|| response.tool_request_id().to_string());
                    Some(json!({
                        "type": "function_call_output",
                        "id": event.id().to_string(),
                        "call_id": call_id,
                        "output": serde_json::to_string(response.outcome())
                            .expect("the tool response outcome should serialize"),
                    }))
                }
                ConversationEventPayload::TurnStart(_)
                | ConversationEventPayload::TurnEnd(_)
                | ConversationEventPayload::ModelRequest(_)
                | ConversationEventPayload::ModelResponse(_)
                | ConversationEventPayload::Automation(_)
                | ConversationEventPayload::Context(_)
                | ConversationEventPayload::Tools(_) => None,
            }
        })
        .collect::<Vec<_>>();
    Value::Array(input)
}

fn replay_reasoning_item(
    specific_event: &ModelSpecificEvent,
    model_request_sources: &HashMap<ConversationEventId, ModelSource>,
    current_source: &ModelSource,
) -> Option<Value> {
    if specific_event.provider_event_type() != REASONING_ITEM_EVENT_TYPE
        || specific_event.provider_payload_version() != PROVIDER_PAYLOAD_VERSION
    {
        return None;
    }
    let origin = model_request_sources.get(&specific_event.model_request_id())?;
    if origin != current_source {
        return None;
    }
    let payload = specific_event.payload();
    let item_id = payload.get("item_id").and_then(Value::as_str)?;
    let summary = payload.get("summary").and_then(Value::as_array)?;
    if !summary.iter().all(is_summary_text) {
        return None;
    }
    let mut item = json!({ "type": "reasoning", "id": item_id });
    item["summary"] = Value::Array(summary.clone());
    if let Some(encrypted_content) = payload.get("encrypted_content").and_then(Value::as_str) {
        item["encrypted_content"] = Value::String(encrypted_content.to_owned());
    }
    Some(item)
}

fn model_request_sources(
    events: &[ConversationEventRecord],
) -> HashMap<ConversationEventId, ModelSource> {
    events
        .iter()
        .filter_map(
            |conversation_event| match conversation_event.event().payload() {
                ConversationEventPayload::ModelRequest(request) => {
                    Some((conversation_event.event().id(), request.source().clone()))
                }
                _ => None,
            },
        )
        .collect()
}

pub(crate) fn is_summary_text(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("summary_text")
        && value.get("text").and_then(Value::as_str).is_some()
}

pub(crate) fn provider_tool(definition: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": definition.name().as_str(),
        "description": definition.description(),
        "parameters": provider_schema(definition.parameters()),
    })
}

fn provider_schema(schema: &Schema) -> Value {
    let mut schema = schema.as_value().clone();
    if let Value::Object(schema_object) = &mut schema {
        schema_object.remove("$schema");
    }
    schema
}

fn provider_tool_call_ids(
    events: &[ConversationEventRecord],
) -> HashMap<ConversationEventId, String> {
    events
        .iter()
        .filter_map(
            |conversation_event| match conversation_event.event().payload() {
                ConversationEventPayload::ToolRequest(request) => provider_tool_call_id(request)
                    .map(|call_id| (conversation_event.event().id(), call_id)),
                _ => None,
            },
        )
        .collect()
}

fn provider_tool_call_id(request: &ToolRequest) -> Option<String> {
    request
        .data()
        .and_then(|data| data.content().get("call_id"))
        .and_then(Value::as_str)
        .filter(|call_id| !call_id.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use serde_json::{Map, json};

    use super::super::PROVIDER_PAYLOAD_VERSION;
    use super::super::tests::{
        conversation_event, reasoning_event, source, tool_definition, turn_and_request, user_event,
    };
    use super::responses_input;
    use tog::conversation::{
        AssistantResponse, Conversation, ConversationEventPayload, ConversationHistory,
        ConversationId, ModelData, ModelId, ModelRequest, ModelSource, ProviderId, ToolName,
        ToolOutcome, ToolRequest, ToolResponse, TurnStart,
    };

    #[test]
    fn responses_input_projects_canonical_events_in_order() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let assistant = AssistantResponse::new(request_id, "Hello.".to_owned())
            .expect("the assistant response should be valid");
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Hello"),
            turn_start,
            model_request,
            conversation_event(
                conversation_id,
                3,
                ConversationEventPayload::AssistantResponse(assistant),
            ),
        ])
        .expect("the conversation should be valid");

        let input = responses_input(conversation.events(), &source());
        assert_eq!(
            input,
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Hello" }]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "id": conversation.events()[3].event().id().to_string(),
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Hello.",
                        "annotations": []
                    }]
                }
            ])
        );
    }

    #[test]
    fn responses_input_replays_reasoning_then_each_model_call_outputs_and_tool_results() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let reasoning_record = conversation_event(
            conversation_id,
            3,
            reasoning_event(
                request_id,
                "rs_1",
                Some("gAAAA-secret"),
                json!([
                    { "type": "summary_text", "text": "Thinking." }
                ]),
            ),
        );
        let assistant_record = conversation_event(
            conversation_id,
            4,
            ConversationEventPayload::AssistantResponse(
                AssistantResponse::new(request_id, "Let me check.".to_owned())
                    .expect("the assistant response should be valid"),
            ),
        );
        let assistant_id = assistant_record.event().id();
        let tool_request_record = conversation_event(
            conversation_id,
            5,
            ConversationEventPayload::ToolRequest(
                ToolRequest::try_new(
                    request_id,
                    ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
                    json!({ "command": "pwd" }),
                    Some(
                        ModelData::new(Map::from_iter([("call_id".to_owned(), json!("call_1"))]))
                            .expect("the model data should be valid"),
                    ),
                )
                .expect("the tool request should be valid"),
            ),
        );
        let tool_request_id = tool_request_record.event().id();
        let tool_response_record = conversation_event(
            conversation_id,
            6,
            ConversationEventPayload::ToolResponse(ToolResponse::new(
                tool_request_id,
                ToolOutcome::succeeded(json!({ "stdout": "/tmp\n" })),
            )),
        );
        let tool_response_id = tool_response_record.event().id();
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Run pwd"),
            turn_start,
            model_request,
            reasoning_record,
            assistant_record,
            tool_request_record,
            tool_response_record,
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            responses_input(conversation.events(), &source()),
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Run pwd" }]
                },
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{ "type": "summary_text", "text": "Thinking." }],
                    "encrypted_content": "gAAAA-secret"
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "id": assistant_id.to_string(),
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Let me check.",
                        "annotations": []
                    }]
                },
                {
                    "type": "function_call",
                    "id": tool_request_id.to_string(),
                    "call_id": "call_1",
                    "name": "shell",
                    "arguments": "{\"command\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "id": tool_response_id.to_string(),
                    "call_id": "call_1",
                    "output": "{\"type\":\"succeeded\",\"value\":{\"stdout\":\"/tmp\\n\"}}"
                }
            ])
        );
    }

    #[test]
    fn responses_input_skips_incompatible_reasoning_items_and_keeps_portable_outputs() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let incompatible_event = conversation_event(
            conversation_id,
            3,
            super::super::tests::reasoning_event_with_version(
                request_id,
                "rs_old",
                PROVIDER_PAYLOAD_VERSION + 1,
                json!([
                    { "type": "summary_text", "text": "Old." }
                ]),
            ),
        );
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Hello"),
            turn_start,
            model_request,
            incompatible_event,
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            responses_input(conversation.events(), &source()),
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Hello" }]
                }
            ]),
            "an incompatible reasoning payload must not be replayed"
        );
    }

    #[test]
    fn responses_input_falls_back_to_the_event_id_when_no_provider_call_id() {
        let conversation_id = ConversationId::new();
        let (turn_start, model_request) = turn_and_request(conversation_id);
        let request_id = model_request.event().id();
        let tool_request_record = conversation_event(
            conversation_id,
            3,
            ConversationEventPayload::ToolRequest(
                ToolRequest::try_new(
                    request_id,
                    ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
                    json!({ "command": "pwd" }),
                    None,
                )
                .expect("the tool request should be valid"),
            ),
        );
        let tool_request_id = tool_request_record.event().id();
        let tool_response_record = conversation_event(
            conversation_id,
            4,
            ConversationEventPayload::ToolResponse(ToolResponse::new(
                tool_request_id,
                ToolOutcome::succeeded(json!({ "stdout": "/tmp\n" })),
            )),
        );
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Run pwd"),
            turn_start,
            model_request,
            tool_request_record,
            tool_response_record,
        ])
        .expect("the conversation should be valid");

        let input = responses_input(conversation.events(), &source());
        assert_eq!(
            input[1]["call_id"],
            tool_request_id.to_string(),
            "the function call must fall back to the event id when no native call id exists"
        );
        assert_eq!(
            input[2]["call_id"],
            tool_request_id.to_string(),
            "the tool result must correlate through the same event id fallback"
        );
    }

    #[test]
    fn responses_input_skips_reasoning_from_a_different_model() {
        let conversation_id = ConversationId::new();
        let turn_start = conversation_event(
            conversation_id,
            1,
            ConversationEventPayload::TurnStart(TurnStart::new(None, 0)),
        );
        let previous_source = ModelSource::new(
            ProviderId::from_str("openrouter").expect("the provider should be valid"),
            ModelId::from_str("deepseek/deepseek-v4-pro").expect("the model should be valid"),
        );
        let model_request = conversation_event(
            conversation_id,
            2,
            ConversationEventPayload::ModelRequest(
                ModelRequest::new(
                    turn_start.event().id(),
                    previous_source,
                    1,
                    Vec::new(),
                    None,
                    None,
                )
                .expect("the model request should be valid"),
            ),
        );
        let request_id = model_request.event().id();
        let reasoning_record = conversation_event(
            conversation_id,
            3,
            reasoning_event(
                request_id,
                "rs_prev",
                Some("gAAAA-prev"),
                json!([{ "type": "summary_text", "text": "Old." }]),
            ),
        );
        let conversation = ConversationHistory::from_events(vec![
            user_event(conversation_id, 0, "Hello"),
            turn_start,
            model_request,
            reasoning_record,
        ])
        .expect("the conversation should be valid");

        assert_eq!(
            responses_input(conversation.events(), &source()),
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Hello" }]
                }
            ]),
            "a previous model's reasoning state must not be replayed to the current model"
        );
    }

    #[test]
    fn provider_tools_translate_definitions_without_dropping_schema_metadata() {
        let definition = tool_definition("shell");

        let provider_tool = super::provider_tool(&definition);

        assert_eq!(provider_tool["type"], "function");
        assert_eq!(provider_tool["name"], "shell");
        assert_eq!(provider_tool["description"], "Run a command.");
        assert_eq!(
            provider_tool["parameters"],
            json!({
                "type": "object",
                "properties": { "command": { "type": "string" } }
            })
        );
        assert!(
            provider_tool["parameters"].get("$schema").is_none(),
            "the provider parameter schema must drop record schema metadata"
        );
        assert!(
            definition.parameters().as_value().get("$schema").is_some(),
            "the recorded definition must keep its schema metadata"
        );
    }
}
