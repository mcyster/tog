mod shell;

pub(crate) use shell::ShellTool;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::conversation_event::{ToolDefinition, ToolExecutionProblem, ToolOutcome, ToolRequest};

pub(crate) trait ExecutableTool: Send + Sync {
    fn definition(&self) -> &ToolDefinition;

    fn execute<'execute>(
        &'execute self,
        arguments: Value,
    ) -> BoxFuture<'execute, Result<Value, ToolExecutionProblem>>;
}

#[derive(Default)]
pub(crate) struct ToolRegistry {
    tools: Vec<Box<dyn ExecutableTool>>,
}

impl ToolRegistry {
    pub(crate) fn register(&mut self, tool: impl ExecutableTool + 'static) {
        self.tools.push(Box::new(tool));
    }

    pub(crate) fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| tool.definition().clone())
            .collect()
    }

    pub(crate) fn execute(&self, request: &ToolRequest) -> BoxFuture<'_, ToolOutcome> {
        let tool_name = request.tool_name().clone();
        let arguments = request.arguments().clone();
        async move {
            let Some(tool) = self
                .tools
                .iter()
                .find(|tool| tool.definition().name() == &tool_name)
            else {
                return ToolOutcome::Problem {
                    problem: ToolExecutionProblem::unknown_tool(tool_name),
                };
            };
            match tool.execute(arguments).await {
                Ok(value) => ToolOutcome::Result { value },
                Err(problem) => ToolOutcome::Problem { problem },
            }
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use futures_util::FutureExt;
    use futures_util::future::BoxFuture;
    use schemars::{JsonSchema, json_schema};
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    use super::{ExecutableTool, ToolRegistry};
    use crate::conversation_event::{
        ModelInvocationId, ToolCallId, ToolDefinition, ToolExecutionProblem,
        ToolExecutionProblemKind, ToolName, ToolOutcome, ToolRequest,
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
        ) -> BoxFuture<'execute, Result<Value, ToolExecutionProblem>> {
            async move {
                let parameters: EchoParameters =
                    serde_json::from_value(arguments).map_err(|error| {
                        ToolExecutionProblem::try_invalid_arguments(error.to_string())
                            .expect("the invalid-argument message should be valid")
                    })?;
                Ok(json!({ "echo": parameters.message }))
            }
            .boxed()
        }
    }

    fn request(tool_name: &str, arguments: Value) -> ToolRequest {
        ToolRequest::try_new(
            ToolCallId::new(),
            ToolName::try_new(tool_name.to_owned()).expect("the tool name should be valid"),
            arguments,
            ModelInvocationId::new(),
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
            ToolOutcome::Result {
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
            ToolOutcome::Problem { problem }
                if problem.kind() == ToolExecutionProblemKind::InvalidArguments
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
            ToolOutcome::Problem { problem }
                if problem.kind() == ToolExecutionProblemKind::UnknownTool
                    && problem.message() == "unknown tool: missing"
        ));
    }
}
