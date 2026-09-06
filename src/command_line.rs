use std::ffi::OsString;
use std::io::{self, Write};

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::conversation::{
    ConversationFact, ConversationId, ConversationProblem, ModelEventImportance, ModelId,
};
use crate::conversation_session::{
    ConversationSession, ConversationSessionProgress, ConversationSessionResult,
};
use crate::openai::OpenAiModelDriver;
use crate::persistence::EventStore;

#[derive(Debug, Parser)]
#[command(
    name = "tog",
    version,
    about = "Command-line access to agentic services",
    disable_help_subcommand = true,
    override_usage = "tog [:turn] [OPTIONS] <USER_PROMPT>...",
    after_help = "When no command is specified, :turn is used."
)]
pub(crate) struct CommandLine {
    #[command(subcommand)]
    command: Command,
}

impl CommandLine {
    pub(crate) fn parse_with_default_command() -> Self {
        let mut arguments: Vec<OsString> = std::env::args_os().collect();
        let first_argument = arguments.get(1).and_then(|argument| argument.to_str());
        let has_command_or_root_option = first_argument.is_some_and(|argument| {
            argument.starts_with(':') || matches!(argument, "--help" | "-h" | "--version" | "-V")
        });
        if !has_command_or_root_option {
            arguments.insert(1, OsString::from(":turn"));
        }
        Self::parse_from(arguments)
    }

    pub(crate) async fn execute(self) -> ConversationSessionResult<()> {
        match self.command {
            Command::Turn(arguments) => {
                let user_prompt = arguments.user_prompt_words.join(" ").parse()?;
                let verbosity = arguments.verbosity;
                let event_store = EventStore::from_environment()?;
                let model_driver = Box::new(OpenAiModelDriver::from_environment(arguments.model)?);
                let conversation_session = match arguments.conversation {
                    Some(conversation_id) => {
                        ConversationSession::open(conversation_id, event_store, model_driver)?
                    }
                    None => ConversationSession::create(event_store, model_driver),
                };
                conversation_session.add_user_request(user_prompt)?;
                eprintln!("#> conversation {}", conversation_session.id());
                conversation_session
                    .invoke(|progress| {
                        match progress {
                            ConversationSessionProgress::InvocationStarted { model } => {
                                eprintln!("## waiting for model {model}");
                            }
                            ConversationSessionProgress::EventCompleted { event } => {
                                render_model_event(&event, verbosity)?;
                            }
                            ConversationSessionProgress::ProblemCompleted { problem } => {
                                render_model_problem(&problem)?;
                            }
                        }
                        Ok(())
                    })
                    .await
            }
        }
    }
}

fn render_model_event(event: &ConversationFact, verbosity: Verbosity) -> io::Result<()> {
    let (message, importance, prefix) = match event {
        ConversationFact::Assistant { response, .. } => {
            (response.message(), ModelEventImportance::Important, "")
        }
        ConversationFact::Communication { communication, .. } => {
            (communication.message(), communication.importance(), "### ")
        }
        _ => return Ok(()),
    };
    if !verbosity.includes(importance) {
        return Ok(());
    };
    let mut standard_output = io::stdout().lock();
    writeln!(standard_output, "{prefix}{message}")?;
    standard_output.flush()
}

fn render_model_problem(problem: &ConversationProblem) -> io::Result<()> {
    let mut standard_output = io::stdout().lock();
    writeln!(standard_output, "### {}", problem.message())?;
    standard_output.flush()
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(
        name = ":turn",
        override_usage = "tog [:turn] [OPTIONS] <USER_PROMPT>..."
    )]
    Turn(TurnArguments),
}

#[derive(Debug, Args)]
struct TurnArguments {
    #[arg(long)]
    conversation: Option<ConversationId>,

    #[arg(long, default_value = "gpt-5.6")]
    model: ModelId,

    #[arg(long, value_enum, default_value = "low")]
    verbosity: Verbosity,

    #[arg(value_name = "USER_PROMPT", num_args = 1.., required = true)]
    user_prompt_words: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Verbosity {
    Low,
    Medium,
    High,
}

impl Verbosity {
    fn includes(self, importance: ModelEventImportance) -> bool {
        match self {
            Self::Low => importance >= ModelEventImportance::Important,
            Self::Medium => importance >= ModelEventImportance::Interesting,
            Self::High => true,
        }
    }
}
