use futures_util::FutureExt;
use futures_util::future::BoxFuture;

use super::{ExecutableTool, ToolRegistry};
use tog::conversation::{FailureCategory, OperationFailure, ToolDefinition, ToolOutcome};

impl ToolRegistry {
    pub fn register(&mut self, tool: impl ExecutableTool + 'static) {
        self.tools.push(Box::new(tool));
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| tool.definition().clone())
            .collect()
    }

    pub fn execute(&self, request: &tog::conversation::ToolRequest) -> BoxFuture<'_, ToolOutcome> {
        let tool_name = request.tool_name().clone();
        let arguments = request.arguments().clone();
        async move {
            let Some(tool) = self
                .tools
                .iter()
                .find(|tool| tool.definition().name() == &tool_name)
            else {
                let failure = OperationFailure::try_new(
                    FailureCategory::UnknownTool,
                    format!("unknown tool: {tool_name}"),
                    None,
                )
                .expect("the unknown-tool failure should be valid");
                return ToolOutcome::Failed { failure };
            };
            match tool.execute(arguments).await {
                Ok(value) => ToolOutcome::Succeeded { value },
                Err(failure) => ToolOutcome::Failed { failure },
            }
        }
        .boxed()
    }
}
