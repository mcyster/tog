mod prompt;

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::asset_store::{AssetMetadata, AssetStore, FileAssetStore, MimeType};
use crate::conversation::ConversationId;
use crate::conversation_event::{
    ConversationEventRecord, ConversationFact, ConversationMessage, ConversationProblem,
    ModelEventImportance, ModelId, TurnOutcome, UserContent,
};
use crate::conversation_event_store::{ConversationEventStore, FileEventStore};
use crate::conversation_session::{
    ConversationSession, ConversationSessionProgress, ConversationSessionResult,
};
use crate::openai::OpenAiModelDriver;
use crate::tools::{ShellTool, ToolRegistry};
use prompt::UserPrompt;

#[derive(Debug, Parser)]
#[command(
    name = "tog",
    version,
    about = "Command-line access to agentic services",
    disable_help_subcommand = true,
    override_usage = "tog [:turn] [OPTIONS] <USER_PROMPT>...\n       tog :log [CONVERSATION_ID]\n       tog :asset add [OPTIONS] <SOURCE_PATH>\n       tog :asset list",
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
                let user_prompt: UserPrompt = arguments.user_prompt_words.join(" ").parse()?;
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
                conversation_session
                    .add_user_request(vec![UserContent::Text(user_prompt.text().to_owned())])?;
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
            Command::Asset(arguments) => {
                let asset_store = FileAssetStore::from_environment()?;
                match arguments.command {
                    AssetCommand::Add(add_arguments) => {
                        let name = match add_arguments.name {
                            Some(name) => name,
                            None => default_asset_name(&add_arguments.source_path)?,
                        };
                        let mime_type = match add_arguments.mime_type {
                            Some(mime_type) => mime_type,
                            None => inferred_mime_type(&add_arguments.source_path)?,
                        };
                        let source = std::fs::File::open(&add_arguments.source_path)?;
                        let asset_id =
                            asset_store.add(name, mime_type, Box::new(source) as Box<dyn Read>)?;
                        let metadata = asset_store.metadata(asset_id)?;
                        let standard_output = io::stdout();
                        let mut standard_output = standard_output.lock();
                        write_asset_metadata(&metadata, &mut standard_output)?;
                        Ok(CommandOutcome::AssetAdded)
                    }
                    AssetCommand::List(_) => {
                        let metadatas = asset_store.list()?;
                        let standard_output = io::stdout();
                        let mut standard_output = standard_output.lock();
                        for metadata in metadatas {
                            write_asset_metadata(&metadata, &mut standard_output)?;
                        }
                        Ok(CommandOutcome::AssetsListed)
                    }
                }
            }
        }
    }
}

pub(crate) enum CommandOutcome {
    Turn(TurnOutcome),
    ConversationLogged,
    AssetAdded,
    AssetsListed,
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

fn write_asset_metadata(metadata: &AssetMetadata, output: &mut impl Write) -> io::Result<()> {
    let value = serde_json::json!({
        "id": metadata.id().to_string(),
        "name": metadata.name(),
        "mime_type": metadata.mime_type().as_str(),
        "byte_size": metadata.byte_size(),
    });
    serde_json::to_writer(&mut *output, &value).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn default_asset_name(source_path: &Path) -> io::Result<String> {
    let Some(file_name) = source_path.file_name() else {
        return Err(io::Error::other("the source path has no file name"));
    };
    let Some(file_name_text) = file_name.to_str() else {
        return Err(io::Error::other("the source file name is not valid UTF-8"));
    };
    Ok(file_name_text.to_owned())
}

fn inferred_mime_type(source_path: &Path) -> io::Result<MimeType> {
    let inferred = infer::get_from_path(source_path)?;
    match inferred {
        Some(kind) => MimeType::from_str(kind.mime_type()).map_err(io::Error::other),
        None => MimeType::from_str("application/octet-stream").map_err(io::Error::other),
    }
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

    #[command(
        name = ":asset",
        about = "Manage immutable stored assets",
        override_usage = "tog :asset add [OPTIONS] <SOURCE_PATH>\n       tog :asset list"
    )]
    Asset(AssetArguments),
}

#[derive(Debug, Args)]
struct AssetArguments {
    #[command(subcommand)]
    command: AssetCommand,
}

#[derive(Debug, Subcommand)]
enum AssetCommand {
    #[command(
        about = "Store a file as an immutable asset",
        override_usage = "tog :asset add [OPTIONS] <SOURCE_PATH>"
    )]
    Add(AssetAddArguments),

    #[command(about = "List stored assets as JSON Lines")]
    List(AssetListArguments),
}

#[derive(Debug, Args)]
struct AssetAddArguments {
    /// Path of the file to store.
    #[arg(value_name = "SOURCE_PATH")]
    source_path: PathBuf,

    /// Name to store the asset under; defaults to the source file name.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,

    /// MIME type of the asset; inferred from content when omitted.
    #[arg(long, value_name = "MIME_TYPE")]
    mime_type: Option<MimeType>,
}

#[derive(Debug, Args)]
struct AssetListArguments {}

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
