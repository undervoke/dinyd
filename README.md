# DinyD — doc is not your diary, agent.

> Built for my own projects; issues and pull requests are closed. Fork freely.

`dinyd` ties a claim in a document to the region it restates. When that
region changes, `dinyd check` reports stale evidence. The worker opens the
document, re-judges the claim, and runs `dinyd digest` to record its evidence
again. DinyD checks the reference and digest, not whether the worker read or
judged correctly. Check never prints document text.

DinyD also protects human-owned document spans through agent file-edit
hooks. The project chooses where to run check (a pre-commit hook, CI, or an
agent's routine).

## Install

macOS and Linux:

```sh
curl -LsSf https://github.com/undervoke/dinyd/releases/latest/download/dinyd-installer.sh | sh
```

Windows PowerShell:

```powershell
irm https://github.com/undervoke/dinyd/releases/latest/download/dinyd-installer.ps1 | iex
```

Restart your terminal and GUI agent host after installation so they see the
updated PATH. Git is required for project operations; a consumer needs no
Rust, Go, or Node toolchain.

Each release on
[GitHub Releases](https://github.com/undervoke/dinyd/releases) also has an archive
and a `.sha256` checksum for manual installation:

| Platform | Archive |
|---|---|
| Linux x64 | `dinyd-x86_64-unknown-linux-musl.tar.xz` |
| Linux arm64 | `dinyd-aarch64-unknown-linux-musl.tar.xz` |
| macOS arm64 | `dinyd-aarch64-apple-darwin.tar.xz` |
| macOS x64 | `dinyd-x86_64-apple-darwin.tar.xz` |
| Windows x64 | `dinyd-x86_64-pc-windows-msvc.zip` |

```sh
v=v0.3.1 t=x86_64-unknown-linux-musl
curl -fsSLO "https://github.com/undervoke/dinyd/releases/download/$v/dinyd-$t.tar.xz"
curl -fsSLO "https://github.com/undervoke/dinyd/releases/download/$v/dinyd-$t.tar.xz.sha256"
shasum -a 256 -c "dinyd-$t.tar.xz.sha256"
tar -xJf "dinyd-$t.tar.xz"   # binary at dinyd-$t/dinyd
```

With Rust 1.93.0 or later and a C build toolchain,
`cargo install --locked --git https://github.com/undervoke/dinyd --tag v0.3.1`
builds the same version from source. The prebuilt archives need neither.

The generated bootstrap installers do not guarantee checksum verification
on every platform. Manual verification above and DinyD's own runtime
cache and self-update downloads verify the archive's full SHA-256.

### Project setup and versions

```sh
cd your-project
dinyd init
```

Init pins one exact version in `.dinyd-version` and offers an agent-host
checklist. Existing DinyD registrations are preselected. Commit the pin and
generated project settings so clones use the same version and bindings.
A clone runs `dinyd init` to prepare its pinned runtime; ordinary setup keeps
the existing pin.

```sh
dinyd init --hosts claude-code,codex,cursor
dinyd init --hosts none
dinyd init --version latest
dinyd init --version 0.3.1
dinyd update
```

`--hosts` selects the desired bindings (`claude-code`, `codex`, `opencode`,
`cursor`, `pi`, `kilo`), or `none` to remove DinyD's bindings. Without a
terminal or `--hosts`, init preserves the existing set; a first unattended
setup pins only. Other settings are preserved. Init does not wire check into
Git hooks or CI.

`init --version` explicitly changes the project's version and matching
bindings. `update` installs the latest stable version over the invoked
installation without changing any project pin. Cached runtimes cannot
self-update. Ordinary commands fetch a missing pinned runtime; a guard
instead denies the edit with an instruction to run init.

Run project commands inside a Git working tree; `dinyd update` also works outside one.

## Anchors

A reference lives inline in the document, next to the claim. Nothing goes in
code files or in a separate mapping file.

```md
Passwords are verified in constant time. <!-- dinyd: src/Hasher.cs#PasswordHasher.Verify s1:3f9a0c1d2e4b5a6c -->
```

`dinyd:` is a **digest anchor**: the claim restates facts of the target, and
the digest records the target as it was when the claim was last judged.

Documents are tracked Markdown files (and [Mermaid](#mermaid) files). An
anchor is an HTML comment anywhere outside code blocks and inline code.

### Document units

A document can be the target too. To reference part of one, wrap that part
in the target document with a start and an end that repeat one name:

```md
Tokens expire after an hour. <!-- dinyd-id: refresh -->Refresh rotates the token.<!-- /dinyd-id: refresh --> Logout revokes it.
```

The unit is the bytes between the two. A boundary alone on its line drops
that whole line; an inline boundary stops at the comment itself. Units may
nest or overlap. A name is ASCII letters, digits, `_`, `.`, and `-`, and must
be declared exactly once per document; declarations follow the same rules as
anchors.

### Links

A link is a reference too, with no anchor needed: every Markdown link, image,
and own-line
[Markdown Preview Enhanced](https://shd101wyy.github.io/markdown-preview-enhanced/)
`@import "path"` must name a path that exists in the same view as the
document.

```md
Read [Checks](verification/checks.md), then the [migrations](../../db/migrations/).

@import "checks.csv"
```

- A link destination is relative to the document's directory, as the
  renderer reads it; a leading `/` starts at the repository root. Selectors,
  by contrast, are always repository-root paths.
- Its `?query` and `#fragment` are ignored, and the rest is percent-decoded
  once. An `@import` path is taken literally.
- A trailing `/` asks for a directory; without one, a file or a directory
  will do. A path that climbs above the repository root does not resolve.
- URLs, same-document `#fragment` links, and anything in code, frontmatter,
  or raw HTML (an HTML comment included) are not checked. To mention a path
  that should not be checked, write it as inline code.

### Bound anchors

A digest anchor whose selector names no path takes the path of the Markdown
link or image right before it; only spaces or tabs may stand between them:

```md
See [workspace departments](adr/031.md#L30-L45)<!-- dinyd: #departments d1:5be0c9e2a1f47d38 -->.
```

- It inherits the path through [link resolution](#links); the comment
  supplies `#location`, or nothing for the whole file. A reference link binds
  where it is used.
- A path-less anchor that follows no link is malformed.
- check prints a bound anchor's selector, and digest's `--expect-selector`
  takes it, as the repository path and location, such as
  `docs/ADR Notes.md#departments`.

### Selectors

`path#location`. The path is the Git path from the repository root, with `/`
as the only separator. Without `#location` the selector is the whole file.

| Target | Location |
|---|---|
| C#, TypeScript, JavaScript, Go | Symbol path: `Class.Method`, `Namespace.Type.Field`, `Type.Method` (Go), `Enum.Member` |
| Vue | Symbol path inside a top-level `<script>` or `<script setup>` |
| JSON, YAML | JSON Pointer: `/scripts/build`, `/jobs/test/steps/0` |
| CSS | A rule's selector: `.card > .title` |
| Document | A unit's name: `docs/auth.md#refresh` |

C# namespaces are not part of the path; TypeScript namespaces and modules
are. Members that share a name (overloads) are ambiguous; `Verify(*)` takes
all of them, in source order, as one region.

### Digest modes

- `s1` (structural): the target's tokens and nesting. Whitespace between
  tokens is ignored; comments count, and text inside strings and comments
  is kept exactly. The default for the code and data formats above.
- `b1` (bytes): the exact bytes with CRLF turned into LF. The default for
  other files and whole `.vue` files; `apply --bytes` chooses it anywhere
  but a document unit.
- `d1` (document unit): the unit's bytes with CRLF turned into LF and dinyd
  comments taken out — a comment alone on its line with that line, a
  declaration or lock marker by itself, an anchor with the one space before it, if any. So adding,
  removing, or updating anchors and inner units never changes it, while any
  other edit inside the unit does. The only mode for document units.

A digest is the first 16 hex digits of SHA-256, and check recomputes it with
the mode recorded in the anchor.

### Mermaid

`*.mermaid` and `*.mmd` files are documents too. In them an anchor or a unit
boundary is an own-line `%%` comment after any leading frontmatter, apply
writes the anchor on a new line after line `n`, and links are not checked.

## Locked sections

Wrap human-owned Markdown source with:

```md
<!-- dinyd-lock -->
Only a human may change this policy.
<!-- /dinyd-lock -->
```

Markers may also be inline. In `.mermaid` and `.mmd`, use own-line
`%% dinyd-lock` and `%% /dinyd-lock`. Literal code examples and Mermaid
frontmatter follow the same exclusions as references. Multiple and nested
locks need no names.

The guard preserves the existing locked bytes, including the marker
comments, and their relative order. It permits edits outside them and
intact moves within the same file. It denies deleting or moving a locked
file, changing a locked span, making its markers disappear into a code
example, or an edit it cannot reconstruct exactly. One failing file blocks
the entire tool call. Broken lock markers in an existing file require human
repair. A new file may introduce locks.

The agent receives a short denial:

```text
docs/policy.md: locked section. Leave it unchanged.
```

`dinyd init` installs bindings for Claude Code, Codex Desktop, OpenCode
v1 >= 1.18.29 and v2, Cursor Agent, Pi, and current Kilo Code. In Orca, select
the agents it launches. Complete the host's trust/reload steps after init.
The guard handles the hosts' file-edit tools; it does not intercept shell
writes or Cursor Tab completions. It is an editing guardrail, not a sandbox.

## Commands

### check

```sh
dinyd check [--index | --worktree | --rev <commit>] [--changed [--base <rev>]]
```

Judges every anchor and local link destination and prints one line per
failure; exit 1 if any fail.

```
docs/auth.md → src/Hasher.cs#PasswordHasher.Verify: stale — re-judge the claim, then run digest
```

| Verdict | Meaning | Next step |
|---|---|---|
| `stale` | The target changed since the digest | Re-judge the claim, then `digest` |
| `undeclared` | A hand-written digest anchor without a digest | `digest` fills it |
| `unresolved` | The target is missing, ambiguous, of a disallowed kind (such as a symlink), or cannot be parsed; a unit is not declared exactly once or its end comes first; a link names no existing path | Re-judge the claim and the reference |
| `malformed` | The anchor or unit declaration itself is broken | Re-judge the claim and the reference |

A failing link prints `link` and its destination as written, once per
document:

```
docs/guide.md → link verification/checks.md: unresolved — re-judge the claim and the reference
```

Output never carries line numbers, digests, or document text; the reason a
target did not resolve goes to stderr.

- `--index` (default) reads documents and targets from the index: the gate
  view. `--worktree` reads the disk: the view while working. `--rev` reads a
  commit. Documents and targets always come from the same view.
- `--changed` judges only anchors in changed documents and anchors whose
  target path changed, including deletions and renames, against `HEAD` or
  `--base`; a link is judged when its document or its path changed. With
  `--rev`, `--base` is required. Malformed anchors are always judged.

### apply

```sh
dinyd apply <doc> --line <n> --selector <selector> [--bytes] [--index | --worktree | --rev <commit>]
```

Writes one anchor with its mode and first digest. The first link ending on
line `n` that names the selector's path takes the anchor; with no such link,
or when that link is already bound, it goes at the end of line `n`. It prints
the anchor and names the view the target was read from. It refuses, writing
nothing, when the target does not resolve, the same anchor is already there,
or check's parser would not read the written anchor back, as inside a code
block.

### digest

```sh
dinyd digest <doc> --line <n> --expect-selector <selector> [--index | --worktree | --rev <commit>]
```

Recomputes the digest of the one digest anchor on line `n` whose selector is
`<selector>`, with its recorded mode, and rewrites only the digest. It prints
the new `mode:digest`, or `unchanged`. To change the mode, edit it in the
anchor by hand and run digest.

Line numbers count from 1 by LF. apply and digest write only the anchor and
keep every other byte, CRLF included. After either, stage the document before
judging with `--index`.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | check: every checked reference is ok; apply, digest: written or unchanged |
| 1 | check: at least one failure; apply, digest: refused, nothing written |
| 2 | Usage or read error |

## License

MIT OR Apache-2.0
