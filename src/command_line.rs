use std::ffi::OsString;
use std::io::{self, Write};

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::conversation::{
    ConversationEventRecord, ConversationFact, ConversationId, ConversationMessage,
    ConversationProblem, ModelEventImportance, ModelId, TurnOutcome,
};
use crate::conversation_session::{
    ConversationSession, ConversationSessionProgress, ConversationSessionResult,
};
use crate::openai::OpenAiModelDriver;
use crate::persistence::{ConversationEventStore, FileEventStore};
use crate::tools::{ShellTool, ToolRegistry};

#[derive(Debug, Parser)]
#[command(
    name = "tog",
    version,
    about = "Command-line access to agentic services",
    disable_help_subcommand = true,
    override_usage = "tog [:turn] [OPTIONS] <USER_PROMPT>...\n       tog :log [CONVERSATION_ID]",
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

    pub(crate) async fn execute(self) -> ConversationSessionResult<CommandOutcome> {
        match self.command {
            Command::Turn(arguments) => {
                let user_prompt = arguments.user_prompt_words.join(" ").parse()?;
                let verbosity = arguments.verbosity;
                let event_store = FileEventStore::from_environment()?;
                let model_driver = Box::new(OpenAiModelDriver::from_environment(arguments.model)?);
                let mut tool_registry = ToolRegistry::default();
                tool_registry.register(ShellTool::new());
                let conversation_session = match arguments.conversation {
                    Some(conversation_id) => ConversationSession::open(
                        conversation_id,
                        event_store,
                        model_driver,
                        tool_registry,
                    )?,
                    None => ConversationSession::create(event_store, model_driver, tool_registry),
                };
                conversation_session.add_user_request(user_prompt)?;
                eprintln!("#> conversation {}", conversation_session.id());
                let outcome = conversation_session
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
                    .await?;
                Ok(CommandOutcome::Turn(outcome))
            }
            Command::Log(arguments) => {
                let event_store = FileEventStore::from_environment()?;
                let conversation_id = match arguments.conversation_id {
                    Some(conversation_id) => conversation_id,
                    None => event_store.latest_id()?.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotFound, "no conversations found")
                    })?,
                };
                let events = event_store.load(conversation_id)?;
                let standard_output = io::stdout();
                let mut standard_output = standard_output.lock();
                write_conversation_log(&events, &mut standard_output)?;
                Ok(CommandOutcome::ConversationLogged)
            }
        }
    }
}

pub(crate) enum CommandOutcome {
    Turn(TurnOutcome),
    ConversationLogged,
}

fn write_conversation_log(
    events: &[ConversationEventRecord],
    output: &mut impl Write,
) -> io::Result<()> {
    for event in events {
        serde_json::to_writer(&mut *output, event).map_err(io::Error::other)?;
        output.write_all(b"\n")?;
    }
    output.flush()
}

fn render_model_event(event: &ConversationFact, verbosity: Verbosity) -> io::Result<()> {
    let (message, importance, prefix) = match event {
        ConversationFact::Message {
            message: ConversationMessage::AssistantResponse { response, .. },
            ..
        } => (response.message(), ModelEventImportance::Important, ""),
        ConversationFact::Message {
            message: ConversationMessage::Communication { communication, .. },
            ..
        } => (communication.message(), communication.importance(), "### "),
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

    #[command(
        name = ":log",
        about = "Dump a conversation log as JSON Lines",
        override_usage = "tog :log [CONVERSATION_ID]"
    )]
    Log(LogArguments),
}

#[derive(Debug, Args)]
struct LogArguments {
    /// Conversation to dump; defaults to the most recently active conversation.
    #[arg(value_name = "CONVERSATION_ID")]
    conversation_id: Option<ConversationId>,
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
