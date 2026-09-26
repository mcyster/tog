use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use super::{ConversationEventId, InvalidOperationFailure, OperationFailure};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct TurnEnd {
    turn_id: ConversationEventId,
    outcome: TurnOutcome,
}

impl TurnEnd {
    pub(crate) fn new(
        turn_id: ConversationEventId,
        outcome: TurnOutcome,
    ) -> Result<Self, InvalidTurnOutcome> {
        let turn_end = Self { turn_id, outcome };
        turn_end.ensure_valid()?;
        Ok(turn_end)
    }

    pub(crate) fn turn_id(&self) -> ConversationEventId {
        self.turn_id
    }

    #[allow(dead_code)]
    pub(crate) fn outcome(&self) -> &TurnOutcome {
        &self.outcome
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidTurnOutcome> {
        if let TurnOutcome::Failed { failure } = &self.outcome {
            failure
                .ensure_valid()
                .map_err(InvalidTurnOutcome::Failure)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TurnOutcome {
    Succeeded,
    Failed { failure: OperationFailure },
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidTurnOutcome {
    Failure(InvalidOperationFailure),
}

impl Display for InvalidTurnOutcome {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failure(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidTurnOutcome {}
