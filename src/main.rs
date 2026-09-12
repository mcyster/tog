mod command_line;
mod conversation;
mod conversation_session;
mod model_driver;
mod openai;
mod persistence;

use std::process::ExitCode;

use command_line::CommandLine;
use conversation::TurnOutcome;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let command_line = CommandLine::parse_with_default_command();
    match command_line.execute().await {
        Ok(TurnOutcome::Succeeded) => ExitCode::SUCCESS,
        Ok(TurnOutcome::Failed) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("Error: {error:?}");
            ExitCode::FAILURE
        }
    }
}
