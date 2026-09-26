# Environment, session, and conversation

A conversation is the durable sequence of events. A session is an initialized,
evolving working context built from an environment. Their lifetimes are separate:
the same conversation can continue across multiple sessions.

This note captures the mental model discussed on 2026-09-26. It does not prescribe
new Rust types or claim these boundaries are already implemented.

## Concepts

| Concept | Responsibility |
| --- | --- |
| Environment | Available resources and configuration: filesystem, project, configuration files, credentials, and external services. |
| Session | Resolved working state: working directory, loaded instructions, effective access policy, selected model, connected tools, and running work. |
| Conversation | Actual events: user input, model requests and responses, tool requests and results, and other recorded activity. |
| Model context | Input assembled for one model invocation from conversation events, applicable instructions, tool definitions, and retrieved information. |
| Memory | Knowledge deliberately retained for reuse beyond its original conversation. |
| History | Previous conversations and recorded session information available to browse, resume, or retrieve. |

History is access to what happened; memory is retained knowledge selected for
future use. Neither automatically becomes model context.

“Model context” is useful here because the input contains more than conversation
content. The conversation remains intact when context selection or compaction
changes what a model sees.

## Where things belong

Something can live in the environment, be resolved into session state, and have
its use recorded in the conversation.

| Item | Environment | Session | Conversation record when relevant |
| --- | --- | --- | --- |
| Access control | Policy definitions | Effective policy and enforcement | Approval requests, decisions, denied actions, and relevant policy identity/version |
| Working directory | Available directories | Current execution directory | Explicit changes and the directory used for an action |
| Agent instructions | `AGENTS.md` and other instruction sources | Discovered and loaded instructions | Contents actually used, or an immutable reference to them |
| Model selection | Provider configuration and credentials | Selected model and effective options | Model and relevant options used for an invocation |
| Tools and MCP | Implementations and server configuration | Connections and available capabilities | Definitions exposed to the model, calls, and results |
| Files | Live filesystem | Access to the working files | Observed reads, writes, and results |
| Memory | Retained knowledge and retrieval facilities | Access to relevant memory | Retrieved material introduced into model context |

Credentials and connection handles remain outside conversation content.
A recorded approval explains an earlier action; whether it grants permission
in a later session is a separate policy choice.

For `AGENTS.md`, distinguish the file on disk, the loaded instructions, and the
instructions actually supplied to a model. Editing the file later must not silently
change the explanation of an earlier invocation. Captured content can be reused
through immutable references rather than copied into every request.

The recording question is: if this value changes tomorrow, do we need its previous
value to understand today's action? Capture the relevant observation or version
at use time, without attempting a complete snapshot of the machine.

## Initialization and continuation

Session initialization resolves configuration and loads baseline instructions.
The session is not frozen: changing directory, selecting another model, reloading
instructions, or connecting a tool changes its working state. Additional scoped
instructions and skills may be loaded as work proceeds.

For example, tomorrow's session can reopen today's conversation using a changed
`AGENTS.md` and current access policy. Earlier events still describe the earlier
work; the next invocation uses the newly assembled context. Resuming the
conversation does not itself restore files, connections, or background processes.

This model leaves reload triggers, session persistence, and the number of
conversations attached to a session open. Those choices need concrete use cases.

## Comparison with other tools

These are conceptual mappings from official documentation reviewed on 2026-09-26,
not claims that their internal types match tog's concepts.

| Tool | Durable conversation vocabulary | Relation to this model |
| --- | --- | --- |
| OpenCode | Session | Its API groups messages and forks under sessions, while configuration and MCP also have separate endpoints. Its “session” includes what tog calls a conversation. |
| Claude Code | Session | Resuming preserves the session/conversation ID while rebuilding some runtime state. Settings files are reread at launch; some launch options must be supplied again. |
| Cursor | Chat/thread | Workspace and chat provide a useful conceptual separation. Rules can apply always, by file scope, by relevance, or manually. |
| Codex | Thread in the app-server API; chat/session in CLI terminology | Stored threads can be read without loading them into memory. A loaded thread has runtime state as well as durable history. |

Sources: [OpenCode server API](https://opencode.ai/docs/server/),
[Claude Code sessions](https://code.claude.com/docs/en/sessions),
[Cursor agent overview](https://cursor.com/docs/agent/overview),
[Cursor rules](https://cursor.com/docs/rules),
[Codex app server](https://learn.chatgpt.com/docs/app-server).

“Resume a session” in another tool can mean “continue a conversation in a newly
initialized session” in this vocabulary.

## Relation to tog

The existing [ConversationSession](../../src/conversation_session.rs) binds a
conversation ID, event store, model driver, and tool registry and orchestrates
execution. It is a starting point for this discussion, not evidence that
environment initialization and context assembly already have separate contracts.

The [conversation event plan](../plans/conversation-design.md) remains responsible
for event vocabulary and the driver boundary. This note does not change that
interface or decide whether instruction snapshots and tool definitions use its
proposed `Context` event. The existing requirement to record model-visible
material or reference it immutably still applies.
