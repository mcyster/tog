# Environment, session, workspace, and conversation

A conversation records actual events and selects the session and working location
used for its work. A session is loaded configuration and capabilities. A workspace
is a view resolved from that session configuration and working location.

This note captures the mental model discussed on 2026-09-26. It describes intended
boundaries, not implemented Rust types or a complete persistence schema.

## Concepts

| Concept | Responsibility |
| --- | --- |
| Environment | Available resources and configuration sources: filesystem, repositories, credentials, tools, and external services. These can change independently of tog. |
| Session | An identified loading of configuration and capabilities, including effective policy, model configuration, and available tools. Runtime implementations and connections support its execution. |
| Workspace | A derived view of the resources, instructions, skills, and tool definitions applicable to a working location under the selected session configuration. |
| Conversation | Durable events recording user input, model and tool activity, session selection, and working-location changes. |
| Model context | Input assembled for one invocation using conversation events, selected session configuration, and the resolved workspace. |
| Memory | Knowledge deliberately retained for reuse beyond its original conversation. |
| History | Previous conversations and recorded activity available to browse, resume, or retrieve. |

History provides access to what happened; memory retains knowledge for future
use. Neither automatically becomes model context. Context selection and compaction
change what the model sees without rewriting the conversation's original events.

## Session selection and workspace resolution

The conversation records its selected `session_id` and working location.
Execution combines that session configuration with the workspace view.
This does not require the session to own the conversation or to contain one
permanent workspace.

Reloading configuration into changed session state produces a new session ID,
recorded in the same conversation's history. Earlier session references retain
their meaning. Changing directory, repository, or git worktree changes the working
location and resolves a new workspace view without requiring a session reload.

Workspace needs no ID for now. Its inputs are the selected session configuration
and working location, with resources read from the environment as needed. The
same inputs may resolve differently after the environment changes; workspace is
not a snapshot.

For example, a conversation starts with session A in a UI repository, then changes
location to a service repository. It continues with session A and the service's
applicable guidance. Later, configuration is reloaded into session B and the
conversation records that selection. All of this remains one conversation.

For operations spanning repositories, applicability follows the target of the
work. Instructions retain their scope, so service guidance does not silently
govern UI edits merely because both have appeared in the conversation.

## Tools, instructions, and skills

The environment supplies implementations and configuration. The session loads
tools and maintains their runtime connections. Workspace resolution determines
which local and shared sources apply to the work.

| Resource | Resolved working state | Conversation record |
| --- | --- | --- |
| Access control | Effective session policy applied to the target operation | Relevant approvals, decisions, and denied actions |
| Working location | Directory, repository, or worktree used to resolve workspace | Location selection and relevant changes |
| Agent instructions | Applicable local and centrally shared guidance | Instructions introduced into model context, directly or by immutable reference |
| Tools and MCP | Session-owned implementations and connections; workspace-applicable toolset | Definitions exposed to an invocation, requests, and results |
| Model configuration | Selected model and effective invocation options | Relevant selection, options, and outcome |
| Memory and skills | Material selected for the current task and scope | Content introduced into model context |

For a model invocation:

1. Resolve the applicable toolset using the selected session and workspace.
2. Select the exposed names, descriptions, and argument schemas.
3. Record that selection in the conversation, directly or by immutable reference.
4. Assemble model context and invoke the model.
5. Execute requested calls using session tools and record their results.

The exposed toolset may be a subset of loaded tools. Implementations, credentials,
and connection handles stay outside model context. Exposing a tool does not itself
authorize execution; current policy can still require approval.

Skills are instruction packages that can be loaded through tools. The loading
tool supplies instructions for subsequent model work. A possible `skill_search`
uses the task and resolved workspace to discover skills from local and central
repositories. Loading applicable baseline `AGENTS.md` instructions does not
depend on an optional skill search.

Separate repositories can share central instructions and skills alongside their
local guidance. Source selection, precedence, and refresh behavior remain choices
to settle when implementing that resolution.

## Storage direction

Use separate storage for identified sessions and conversations:

| Default path | Purpose |
| --- | --- |
| `~/.local/share/tog/sessions/$ID/` | Persisted session configuration and relevant state |
| `~/.local/share/tog/conversations/$ID/` | Events, including session references and working-location changes |

Workspace is derived and requires neither a `workspaces/$ID/` directory nor
a `WorkspaceId` at this stage. Independent workspace identity can be introduced
if a concrete need to save, name, reopen, or share one emerges. Persisting a session
does not persist live processes, connections, or the entire environment.

## What the history promises

Record what happens at tog's boundaries and the inputs, outputs, and configuration
changes useful for understanding the work. Capturing model-visible instructions
and tool definitions explains what was supplied to the model.

This is not complete reproducibility. Files, services, tool internals, and the wider
environment can change without a corresponding tog event. Session identity and
workspace resolution do not freeze those conditions. Unrecorded influences limit
what the history explains; they do not invalidate the model.

Record observed values when useful rather than introducing snapshots or identities
solely because something might change. Resuming a conversation continues its
history using available resources; it does not restore the old environment or
promise identical results.

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
execution. That existing implementation does not establish the ownership model
described here.

The [conversation event plan](../plans/conversation-design.md) remains responsible
for event vocabulary and the driver boundary. Concrete event names for session
and location selection, and the representation of loaded context, remain to be
designed. Recording model-visible material does not imply capturing every
environmental dependency.
