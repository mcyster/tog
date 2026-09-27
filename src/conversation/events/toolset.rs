use serde::{Deserialize, Serialize};

use crate::toolset::{InvalidToolset, Toolset};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ToolsetDeclared {
    toolset: Toolset,
}

impl ToolsetDeclared {
    pub(crate) fn new(toolset: Toolset) -> Self {
        Self { toolset }
    }

    pub(crate) fn toolset(&self) -> &Toolset {
        &self.toolset
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidToolset> {
        self.toolset.ensure_valid()
    }
}
