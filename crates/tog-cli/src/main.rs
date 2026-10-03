mod command_line;

use std::process::ExitCode;

use command_line::{CommandLine, CommandOutcome};
use tog::conversation::TurnOutcome;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let command_line = CommandLine::parse_with_default_command();
    match command_line.execute().await {
        Ok(CommandOutcome::Turn(TurnOutcome::Succeeded))
        | Ok(CommandOutcome::ConversationLogged)
        | Ok(CommandOutcome::AssetAdded)
        | Ok(CommandOutcome::AssetsListed) => ExitCode::SUCCESS,
        Ok(CommandOutcome::Turn(TurnOutcome::Failed { .. })) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}
