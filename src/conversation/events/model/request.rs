use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use super::{InvalidModelData, ModelData, ModelSource};
use crate::conversation::events::ConversationEventId;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ModelRequest {
    turn_id: ConversationEventId,
    source: ModelSource,
    input_through: u64,
    depends_on: Vec<ConversationEventId>,
    retry_of: Option<ConversationEventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    options: Option<ModelData>,
}

impl ModelRequest {
    pub(crate) fn new(
        turn_id: ConversationEventId,
        source: ModelSource,
        input_through: u64,
        depends_on: Vec<ConversationEventId>,
        retry_of: Option<ConversationEventId>,
        options: Option<ModelData>,
    ) -> Result<Self, InvalidModelRequest> {
        let request = Self {
            turn_id,
            source,
            input_through,
            depends_on,
            retry_of,
            options,
        };
        request.ensure_valid()?;
        Ok(request)
    }

    pub(crate) fn turn_id(&self) -> ConversationEventId {
        self.turn_id
    }

    pub(crate) fn input_through(&self) -> u64 {
        self.input_through
    }

    pub(crate) fn depends_on(&self) -> &[ConversationEventId] {
        &self.depends_on
    }

    pub(crate) fn retry_of(&self) -> Option<ConversationEventId> {
        self.retry_of
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidModelRequest> {
        if let Some(options) = &self.options {
            options
                .ensure_valid()
                .map_err(InvalidModelRequest::ModelData)?;
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidModelRequest {
    ModelData(InvalidModelData),
}

impl Display for InvalidModelRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelData(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidModelRequest {}
