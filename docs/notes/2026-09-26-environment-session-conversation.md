# Environment, session, workspace, and conversation

A conversation is the durable sequence of events. A session is an initialized,
evolving runtime built from an environment. Within that session, a workspace
identifies where work applies and resolves the relevant resources. Changing a
directory, repository, or git worktree updates the workspace without rebuilding
the session. The same conversation can continue across workspace changes and
across multiple sessions.

This note captures the mental model discussed on 2026-09-26. It does not prescribe
new Rust types or claim these boundaries are already implemented.

## Concepts

| Concept | Responsibility |
| --- | --- |
| Environment | Available resources and configuration: filesystem, project, configuration files, credentials, and external services. |
| Session | Loaded configuration, effective access policy, selected model, tool implementations and connections, and ongoing work. Owns the working lifetime. |
| Workspace | Current working references: repository, git worktree, directory, and applicable local and shared instruction, skill, and tool sources. Lives within the session. |
| Conversation | Actual events: user input, model requests and responses, tool requests and results, and other recorded activity. |
| Model context | Input assembled for one model invocation from conversation events, session configuration, workspace-applicable instructions and tool definitions, and retrieved information. |
| Memory | Knowledge deliberately retained for reuse beyond its original conversation. |
| History | Previous conversations and recorded session information available to browse, resume, or retrieve. |

History is access to what happened; memory is retained knowledge selected for
future use. Neither automatically becomes model context.

“Model context” is useful here because the input contains more than conversation
content. The conversation remains intact when context selection or compaction
changes what a model sees.

## Where things belong

The environment defines what is available. The session loads and maintains
working resources; the workspace determines which sources and definitions apply
to the current work. Their use is recorded in the conversation.

| Item | Environment | Session | Conversation record when relevant |
| --- | --- | --- | --- |
| Access control | Policy definitions | Effective policy and enforcement | Approval requests, decisions, denied actions, and relevant policy identity/version |
| Working directory | Available directories and worktrees | Current workspace points to the working location | Explicit changes and the directory used for an action |
| Agent instructions | Local and centrally shared instruction sources | Workspace resolves applicability; session loads instructions | Contents actually used, or an immutable reference to them |
| Model selection | Provider configuration and credentials | Selected model and effective options | Model and relevant options used for an invocation |
| Tools and MCP | Implementations and server configuration | Session owns loaded tools and connections; workspace resolves the applicable toolset | Definitions exposed to the model, calls, and results |
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

## Workspace changes within a session

Session initialization loads configuration and establishes baseline resources.
The workspace holds references to where the session is working and which sources
apply there. Updating those references can trigger loading or refreshing local
instructions, skills, and tool definitions. Existing session resources remain
available subject to the effective policy.

For example, a session starts in a UI repository and then works in a service
repository. Its workspace changes to the service repository and directory.
The session loads the service's applicable instructions and skills while retaining
its conversation and runtime. Instructions retain their scope: service guidance
must not silently become guidance for subsequent UI work.

Separate repositories can use a central repository for shared instructions and
skills alongside their local guidance. The workspace resolves those configured
sources. Version selection, precedence, and automatic refresh rules remain open;
this note does not require a new package manager or repository layout.

A session can use a default workspace, with an explicit target workspace for work
that spans repositories. Whether that requires multiple workspace objects is an
implementation choice. Applicability follows the target of the work, not merely
the shell's current directory.

## Tools and skills reaching the conversation

Tools live in the session as executable implementations and connections. The
workspace resolves which tool sources and definitions apply to the work, and the
session loads what is needed from those sources.

For a model invocation:

1. Resolve the applicable toolset through the workspace and session policy.
2. Select the definitions to expose: names, descriptions, and argument schemas.
3. Record those definitions, or immutable references to them, in the conversation
   before invoking the model.
4. Assemble model context using that recorded selection.
5. Execute model-requested tool calls through session-owned implementations,
   checking current authorization, and record their results.

The exposed toolset may be a subset of the tools loaded in the session.
Implementations, live connections, and credentials are not passed to the model.
Exposure does not itself authorize execution; a request may still require approval.
A workspace change must not silently reinterpret a pending call against a different
tool implementation. The exact binding mechanism remains to be designed.

Skills are reusable instruction packages and can be loaded through tools.
A skill-loading tool returns instructions for the model to follow; it need not
execute the procedure described by those instructions. Loading through a tool
does not require treating skills and executable operations as the same concept.

A possible `skill_search` tool can search local and central sources using the task
and workspace. The session resolves the scope; search uses it. Applicable baseline
`AGENTS.md` instructions must not depend on an optional skill search. Record
selected skill content or immutable references when it enters model context.

## Continuation across sessions

Tomorrow's session can reopen today's conversation using a changed `AGENTS.md`
and current access policy. Earlier events still describe the earlier work; the
next invocation uses the newly resolved workspace and context. Resuming the
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
environment initialization, workspace resolution, and context assembly already
have separate contracts.

The [conversation event plan](../plans/conversation-design.md) remains responsible
for event vocabulary and the driver boundary. This note does not change that
interface or decide whether instruction snapshots and tool definitions use its
proposed `Context` event. The existing requirement to record model-visible
material or reference it immutably still applies.
