# Locked document source

A lock reserves source for human edits; its enforcement boundary is the agent's file-editing tools.
The guard reads current working-tree bytes because those are the bytes the tool will edit.

## Protected source

Markdown uses `<!-- dinyd-lock -->` and `<!-- /dinyd-lock -->`; Mermaid documents use own-line `%% dinyd-lock` and `%% /dinyd-lock`.
The [literal-example exclusions](../README.md#anchors) apply to these markers as well.
A lock protects both boundary comments and the enclosed bytes; multiple and nested locks need no identifiers.
Protection follows the source within its file, so an intact span may move inside that file, while deleting or moving the locked file is denied.
Existing protected spans must survive byte-for-byte, in relative order, as recognized locks.
Re-parsing the post-edit file is necessary because an unclosed code fence can deactivate a marker without changing any of its bytes.
Unchanged patch context, whole-file writes preserving the protected source, and new enclosing locks remain valid.
For a file already marked as locked, malformed markers or an edit that cannot be reconstructed exactly deny the call; the instruction is to repair the markers or re-read the file.
A failing file rejects the whole call before any write, so a multi-file edit cannot partially apply before the guard answers.
The refusal gives only the path and a short instruction, such as `<path>: locked section. Leave it unchanged.`
Lock markers follow unit-marker projection in `d1`, keeping the protection declaration separate from the evidence.
`dinyd-id` selects evidence; lock ownership is independent of [evidence references](anchors.md).

## Host boundary

The supported hosts are [Claude Code](https://code.claude.com/docs/en/hooks), [Codex Desktop](https://learn.chatgpt.com/docs/hooks), [OpenCode v1 and v2](https://opencode.ai/v2/docs/build/plugins/migrate-v1/), [Cursor Agent](https://cursor.com/docs/hooks), [Pi](https://pi.dev/docs/latest/extensions), and [current Kilo Code](https://kilo.ai/docs/automate/extending/plugins).
Orca uses the binding of the agent it launches.
Adapters translate native tool calls and denials; lock policy and edit reconstruction belong to the Rust runtime.
Plugin adapters run in the host's existing JavaScript runtime, preserving native-binary installation for the consuming project.
OpenCode v1 >= 1.18.29 and v2 share one adapter with both `server` and `setup`, so installing both generations cannot double-run the guard.
Cursor presents edits as full-content `Write` calls and delivers a denial's agent instruction through `user_message`; Tab edits are outside this Agent-tool boundary.
[Installation](installation.md) owns project context and activation, including the host's trust requirements.

@import "lock.mermaid"
