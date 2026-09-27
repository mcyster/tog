use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::conversation::events::{
    ConversationEventId, ConversationEventPayload, ModelEvent, TurnEnd,
};
use crate::conversation::{Conversation, ConversationId};
use crate::conversation_event_store::ConversationEventRecord;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConversationHistory {
    id: ConversationId,
    events: Vec<ConversationEventRecord>,
}

impl ConversationHistory {
    pub(crate) fn from_events(
        events: Vec<ConversationEventRecord>,
    ) -> Result<Self, InvalidConversation> {
        let conversation_id = events
            .first()
            .map(|event| event.event.conversation_id())
            .ok_or(InvalidConversation::Empty)?;

        let mut previous_position = None;
        for event in &events {
            if event.event.conversation_id() != conversation_id {
                return Err(InvalidConversation::MixedConversationIds {
                    expected: conversation_id,
                    found: event.event.conversation_id(),
                });
            }

            if let Some(previous_position) = previous_position
                && event.position <= previous_position
            {
                return Err(InvalidConversation::InvalidPosition {
                    expected: previous_position
                        .checked_add(1)
                        .ok_or(InvalidConversation::TooManyEvents)?,
                    found: event.position,
                });
            }
            previous_position = Some(event.position);

            event
                .ensure_valid()
                .map_err(|error| InvalidConversation::InvalidEvent {
                    position: event.position,
                    reason: error.to_string(),
                })?;
        }

        let final_position = previous_position.ok_or(InvalidConversation::Empty)?;
        validate_references(&events, final_position)?;

        Ok(Self {
            id: conversation_id,
            events,
        })
    }
}

impl Conversation for ConversationHistory {
    fn id(&self) -> ConversationId {
        self.id
    }

    fn events(&self) -> &[ConversationEventRecord] {
        &self.events
    }
}

fn validate_references(
    events: &[ConversationEventRecord],
    final_position: u64,
) -> Result<(), InvalidConversation> {
    let by_id = events
        .iter()
        .map(|event| (event.event.id(), event))
        .collect::<HashMap<_, _>>();

    let mut closed_turns = HashSet::new();
    let mut resolved_model_responses = HashSet::new();
    let mut claimed_outputs = HashSet::new();

    for event in events {
        match event.event.payload() {
            ConversationEventPayload::TurnStart(turn_start) => {
                validate_input_through(event, turn_start.input_through(), final_position)?;
                if let Some(user_id) = turn_start.user_id() {
                    require_kind(&by_id, event, "turn start", user_id, |candidate| {
                        matches!(candidate, ConversationEventPayload::User(_))
                    })?;
                }
            }
            ConversationEventPayload::TurnEnd(turn_end) => {
                validate_turn_end(event, turn_end, &by_id, &mut closed_turns)?;
            }
            ConversationEventPayload::ModelRequest(request) => {
                validate_input_through(event, request.input_through(), final_position)?;
                require_kind(
                    &by_id,
                    event,
                    "model request",
                    request.turn_id(),
                    |candidate| matches!(candidate, ConversationEventPayload::TurnStart(_)),
                )?;
                for dependency in request.depends_on() {
                    require_kind(
                        &by_id,
                        event,
                        "model request dependency",
                        *dependency,
                        |candidate| matches!(candidate, ConversationEventPayload::ToolRequest(_)),
                    )?;
                }
                if let Some(retry_of) = request.retry_of() {
                    let retried = require_kind(
                        &by_id,
                        event,
                        "model request retry",
                        retry_of,
                        |candidate| matches!(candidate, ConversationEventPayload::ModelRequest(_)),
                    )?;
                    let ConversationEventPayload::ModelRequest(retried_request) =
                        retried.event.payload()
                    else {
                        unreachable!("the referenced event kind was just checked");
                    };
                    if retried_request.turn_id() != request.turn_id() {
                        return Err(InvalidConversation::InvalidReference {
                            position: event.position,
                            reason: "model request retries a request from a different turn"
                                .to_owned(),
                        });
                    }
                }
            }
            ConversationEventPayload::AssistantResponse(response) => {
                validate_model_request_reference(&by_id, event, response)?;
            }
            ConversationEventPayload::ToolRequest(request) => {
                validate_model_request_reference(&by_id, event, request)?;
            }
            ConversationEventPayload::ModelSpecificEvent(model_specific_event) => {
                validate_model_request_reference(&by_id, event, model_specific_event)?;
            }
            ConversationEventPayload::ToolResponse(response) => {
                require_kind(
                    &by_id,
                    event,
                    "tool response",
                    response.tool_request_id(),
                    |candidate| matches!(candidate, ConversationEventPayload::ToolRequest(_)),
                )?;
            }
            ConversationEventPayload::ModelResponse(response) => {
                validate_model_request_reference(&by_id, event, response)?;
                let response_model_request_id = response.model_request_id();
                if !resolved_model_responses.insert(response_model_request_id) {
                    return Err(InvalidConversation::InvalidReference {
                        position: event.position,
                        reason: "model request has more than one terminal model response"
                            .to_owned(),
                    });
                }
                for output_event_id in response.output_event_ids() {
                    validate_model_response_output(
                        &by_id,
                        event,
                        response_model_request_id,
                        *output_event_id,
                        &mut claimed_outputs,
                    )?;
                }
            }
            ConversationEventPayload::User(_)
            | ConversationEventPayload::Automation(_)
            | ConversationEventPayload::Context(_)
            | ConversationEventPayload::Tools(_) => {}
        }
    }
    Ok(())
}

type ReferenceTable<'events> = HashMap<ConversationEventId, &'events ConversationEventRecord>;

fn validate_input_through(
    event: &ConversationEventRecord,
    input_through: u64,
    final_position: u64,
) -> Result<(), InvalidConversation> {
    if input_through > final_position {
        return Err(InvalidConversation::InvalidReference {
            position: event.position,
            reason: format!(
                "input position {input_through} is beyond the conversation boundary {final_position}"
            ),
        });
    }
    Ok(())
}

fn validate_turn_end(
    event: &ConversationEventRecord,
    turn_end: &TurnEnd,
    by_id: &ReferenceTable<'_>,
    closed_turns: &mut HashSet<ConversationEventId>,
) -> Result<(), InvalidConversation> {
    require_kind(by_id, event, "turn end", turn_end.turn_id(), |candidate| {
        matches!(candidate, ConversationEventPayload::TurnStart(_))
    })?;
    if !closed_turns.insert(turn_end.turn_id()) {
        return Err(InvalidConversation::InvalidReference {
            position: event.position,
            reason: "turn has more than one terminal turn end event".to_owned(),
        });
    }
    Ok(())
}

fn validate_model_request_reference(
    by_id: &ReferenceTable<'_>,
    event: &ConversationEventRecord,
    model_event: &dyn ModelEvent,
) -> Result<(), InvalidConversation> {
    require_kind(
        by_id,
        event,
        "model event",
        model_event.model_request_id(),
        |candidate| matches!(candidate, ConversationEventPayload::ModelRequest(_)),
    )?;
    Ok(())
}

fn validate_model_response_output(
    by_id: &ReferenceTable<'_>,
    event: &ConversationEventRecord,
    response_model_request_id: ConversationEventId,
    output_event_id: ConversationEventId,
    claimed_outputs: &mut HashSet<ConversationEventId>,
) -> Result<(), InvalidConversation> {
    let output =
        by_id
            .get(&output_event_id)
            .ok_or_else(|| InvalidConversation::InvalidReference {
                position: event.position,
                reason: format!(
                    "model response references missing output conversation event {output_event_id}"
                ),
            })?;
    let output_model_request_id = match output.event.payload() {
        ConversationEventPayload::AssistantResponse(response) => Some(response.model_request_id()),
        ConversationEventPayload::ToolRequest(request) => Some(request.model_request_id()),
        _ => None,
    };
    let Some(output_model_request_id) = output_model_request_id else {
        return Err(InvalidConversation::InvalidReference {
            position: event.position,
            reason: format!("model response output {output_event_id} is not a model output event"),
        });
    };
    if output_model_request_id != response_model_request_id {
        return Err(InvalidConversation::InvalidReference {
            position: event.position,
            reason: format!(
                "model response output {output_event_id} belongs to another model request"
            ),
        });
    }
    if output.position >= event.position {
        return Err(InvalidConversation::InvalidReference {
            position: event.position,
            reason: format!(
                "model response output {output_event_id} was recorded after its terminal response"
            ),
        });
    }
    if !claimed_outputs.insert(output_event_id) {
        return Err(InvalidConversation::InvalidReference {
            position: event.position,
            reason: format!(
                "model output {output_event_id} is claimed by more than one model response"
            ),
        });
    }
    Ok(())
}

fn require_kind<'events>(
    by_id: &ReferenceTable<'events>,
    event: &ConversationEventRecord,
    description: &str,
    referenced_id: ConversationEventId,
    is_expected_kind: impl Fn(&ConversationEventPayload) -> bool,
) -> Result<&'events ConversationEventRecord, InvalidConversation> {
    let candidate =
        by_id
            .get(&referenced_id)
            .ok_or_else(|| InvalidConversation::InvalidReference {
                position: event.position,
                reason: format!(
                    "{description} references missing conversation event {referenced_id}"
                ),
            })?;
    if !is_expected_kind(candidate.event.payload()) {
        return Err(InvalidConversation::InvalidReference {
            position: event.position,
            reason: format!("{description} references unexpected event {referenced_id}"),
        });
    }
    Ok(candidate)
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidConversation {
    Empty,
    MixedConversationIds {
        expected: ConversationId,
        found: ConversationId,
    },
    InvalidPosition {
        expected: u64,
        found: u64,
    },
    InvalidEvent {
        position: u64,
        reason: String,
    },
    InvalidReference {
        position: u64,
        reason: String,
    },
    TooManyEvents,
}

impl Display for InvalidConversation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "a conversation must contain at least one event"),
            Self::MixedConversationIds { expected, found } => write!(
                formatter,
                "conversation event belongs to {found}, expected {expected}"
            ),
            Self::InvalidPosition { expected, found } => {
                write!(
                    formatter,
                    "expected conversation event position {expected}, found {found}"
                )
            }
            Self::InvalidEvent { position, reason } => {
                write!(
                    formatter,
                    "invalid conversation event at position {position}: {reason}"
                )
            }
            Self::InvalidReference { position, reason } => {
                write!(
                    formatter,
                    "invalid conversation reference at position {position}: {reason}"
                )
            }
            Self::TooManyEvents => write!(formatter, "conversation contains too many events"),
        }
    }
}

impl Error for InvalidConversation {}
