mod definition_set;
mod registry;
#[cfg(test)]
mod tests;

use futures_util::future::BoxFuture;
use serde_json::Value;

use tog::conversation::{OperationFailure, Tool, ToolDefinition};

pub struct Toolset {
    tools: Vec<Tool>,
}

pub trait ExecutableTool: Send + Sync {
    fn definition(&self) -> &ToolDefinition;

    fn execute<'execute>(
        &'execute self,
        arguments: Value,
    ) -> BoxFuture<'execute, Result<Value, OperationFailure>>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Box<dyn ExecutableTool>>,
}
