use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use schemars::{JsonSchema, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ExecutableTool, ToolRegistry};
use tog::conversation::{
    ConversationEventId, FailureCategory, OperationFailure, ToolDefinition, ToolName, ToolOutcome,
    ToolRequest,
};

#[derive(Deserialize, JsonSchema, Serialize)]
struct EchoParameters {
    message: String,
}

struct EchoTool {
    definition: ToolDefinition,
}

impl EchoTool {
    fn new() -> Self {
        Self {
            definition: ToolDefinition::try_new(
                ToolName::try_new("echo".to_owned()).expect("the tool name should be valid"),
                "Echo a message.".to_owned(),
                schema_for_parameters(),
                json_schema!({ "type": "object" }),
            )
            .expect("the tool definition should be valid"),
        }
    }
}

fn schema_for_parameters() -> schemars::Schema {
    schemars::schema_for!(EchoParameters)
}

impl ExecutableTool for EchoTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute<'execute>(
        &'execute self,
        arguments: Value,
    ) -> BoxFuture<'execute, Result<Value, OperationFailure>> {
        async move {
            let parameters: EchoParameters =
                serde_json::from_value(arguments).map_err(|error| {
                    OperationFailure::try_new(
                        FailureCategory::InvalidArguments,
                        error.to_string(),
                        None,
                    )
                    .expect("the invalid-argument message should be valid")
                })?;
            Ok(json!({ "echo": parameters.message }))
        }
        .boxed()
    }
}

fn request(tool_name: &str, arguments: Value) -> ToolRequest {
    ToolRequest::try_new(
        ConversationEventId::new(),
        ToolName::try_new(tool_name.to_owned()).expect("the tool name should be valid"),
        arguments,
        None,
    )
    .expect("the tool request should be valid")
}

#[tokio::test]
async fn registry_lists_registered_definitions() {
    let mut registry = ToolRegistry::default();
    registry.register(EchoTool::new());

    let definitions = registry.definitions();

    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name().as_str(), "echo");
}

#[tokio::test]
async fn registry_executes_a_registered_tool() {
    let mut registry = ToolRegistry::default();
    registry.register(EchoTool::new());

    let outcome = registry
        .execute(&request("echo", json!({ "message": "hello" })))
        .await;

    assert_eq!(
        outcome,
        ToolOutcome::Succeeded {
            value: json!({ "echo": "hello" })
        }
    );
}

#[tokio::test]
async fn registry_reports_invalid_arguments_from_a_tool() {
    let mut registry = ToolRegistry::default();
    registry.register(EchoTool::new());

    let outcome = registry.execute(&request("echo", json!({}))).await;

    assert!(matches!(
        outcome,
        ToolOutcome::Failed { failure }
            if failure.category() == FailureCategory::InvalidArguments
    ));
}

#[tokio::test]
async fn registry_reports_an_unknown_tool() {
    let registry = ToolRegistry::default();

    let outcome = registry
        .execute(&request("missing", json!({ "message": "hello" })))
        .await;

    assert!(matches!(
        outcome,
        ToolOutcome::Failed { failure }
            if failure.category() == FailureCategory::UnknownTool
                && failure.message() == "unknown tool: missing"
    ));
}
