mod command_line;
mod conversation;
mod conversation_event_store;
mod conversation_session;
mod model_driver;
mod openai;
mod tools;
mod toolset;

use std::process::ExitCode;

use crate::conversation::TurnOutcome;
use command_line::{CommandLine, CommandOutcome};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let command_line = CommandLine::parse_with_default_command();
    match command_line.execute().await {
        Ok(CommandOutcome::Turn(TurnOutcome::Succeeded))
        | Ok(CommandOutcome::ConversationLogged) => ExitCode::SUCCESS,
        Ok(CommandOutcome::Turn(TurnOutcome::Failed { .. })) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}
