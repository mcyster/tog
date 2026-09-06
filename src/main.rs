mod command_line;
mod conversation;
mod conversation_session;
mod model_driver;
mod openai;
mod persistence;

use command_line::CommandLine;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let command_line = CommandLine::parse_with_default_command();
    command_line.execute().await
}
