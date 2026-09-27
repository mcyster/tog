use crate::conversation::{Tool, ToolAvailability, ToolDefinition, Tools};

pub(crate) struct Toolset {
    tools: Vec<Tool>,
}

impl Toolset {
    pub(crate) fn immediate(definitions: Vec<ToolDefinition>) -> Self {
        Self {
            tools: definitions
                .into_iter()
                .map(|definition| Tool::new(definition, ToolAvailability::Immediate))
                .collect(),
        }
    }

    pub(crate) fn into_tools(self) -> Tools {
        Tools::new(self.tools)
    }
}
