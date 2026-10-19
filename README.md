# DinyD — doc is not your diary, agent.

> Built for my own projects; issues and pull requests are closed. Fork freely.

`dinyd` ties a claim in a document to the code region it restates. When that
code changes, `dinyd check` fails until a worker — person or agent — opens the
document, re-judges the claim, and updates the reference. It never prints
document text, so the only way through is to read the document.

dinyd provides the verdict and the update; where it runs (a pre-commit hook,
CI, an agent's routine) is the project's choice.

## Install

Pin one version per project, so hooks, CI, and every clone judge with the
same binary. Each release on
[GitHub Releases](https://github.com/undervoke/dinyd/releases) has an archive
and a `.sha256` checksum for:

| Platform | Archive |
|---|---|
| Linux x64 | `dinyd-x86_64-unknown-linux-musl.tar.xz` |
| Linux arm64 | `dinyd-aarch64-unknown-linux-musl.tar.xz` |
| macOS arm64 | `dinyd-aarch64-apple-darwin.tar.xz` |
| macOS x64 | `dinyd-x86_64-apple-darwin.tar.xz` |
| Windows x64 | `dinyd-x86_64-pc-windows-msvc.zip` |

```sh
v=v0.1.0 t=x86_64-unknown-linux-musl
curl -fsSLO "https://github.com/undervoke/dinyd/releases/download/$v/dinyd-$t.tar.xz"
curl -fsSLO "https://github.com/undervoke/dinyd/releases/download/$v/dinyd-$t.tar.xz.sha256"
shasum -a 256 -c "dinyd-$t.tar.xz.sha256"
tar -xJf "dinyd-$t.tar.xz"   # binary at dinyd-$t/dinyd
```

With Rust 1.93.0 or later and a C build toolchain,
`cargo install --locked --git https://github.com/undervoke/dinyd --tag v0.1.0`
builds the same version from source. The prebuilt archives need neither.

dinyd runs `git` at run time and must be run inside a Git working tree.

## Anchors

A reference lives inline in the document, next to the claim. Nothing goes in
code files or in a separate mapping file.

```md
Passwords are verified in constant time. <!-- dinyd: src/Hasher.cs#PasswordHasher.Verify s1:3f9a0c1d2e4b5a6c -->

The release rules are owned by the workflow. <!-- dinyd-ref: .github/workflows/release.yml -->
```

- `dinyd:` is a **digest anchor**: the claim restates facts of the target, and
  the digest records the target as it was when the claim was last judged.
- `dinyd-ref:` is a **delegation**: the claim only points at the target, so
  dinyd checks that it exists. It may name a directory.

Documents are tracked Markdown (`*.md`) and Mermaid (`*.mermaid`, `*.mmd`)
files. In Markdown an anchor is an HTML comment anywhere outside code blocks
and inline code. In Mermaid it is an own-line `%%` comment, after any leading
frontmatter.

### Document units

A document can be the target too. To reference part of one, wrap that part
in the target document with a start and an end that repeat one name:

```md
Tokens expire after an hour. <!-- dinyd-id: refresh -->Refresh rotates the token.<!-- /dinyd-id: refresh --> Logout revokes it.
```

```
%% dinyd-id: edge
  A --> B
%% /dinyd-id: edge
```

The unit is the bytes between the two. A boundary alone on its line drops
that whole line; an inline boundary stops at the comment itself. Units may
nest or overlap. A name is ASCII letters, digits, `_`, `.`, and `-`, and must
be declared exactly once per document; declarations follow the same code and
frontmatter rules as anchors.

### Links

A link is a reference too: every Markdown link, image, and own-line
[Markdown Preview Enhanced](https://shd101wyy.github.io/markdown-preview-enhanced/)
`@import "path"` must name a path that exists in the same view as the
document, with no anchor written next to it.

```md
Read [Checks](verification/checks.md), then the [migrations](../../db/migrations/).

@import "checks.mermaid"
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

`dinyd-ref` remains for what a link cannot carry: a location such as a
symbol or a unit, a path mentioned in prose or inline code, and references
in Mermaid documents.

### Selectors

`path#location`. The path is the Git path from the repository root, with `/`
as the only separator. Without `#location` the selector is the whole file.

| Target | Location |
|---|---|
| C#, TypeScript, JavaScript, Go | Symbol path: `Class.Method`, `Namespace.Type.Field`, `Type.Method` (Go), `Enum.Member` |
| Vue | Symbol path inside a top-level `<script>` or `<script setup>` |
| JSON, YAML | JSON Pointer: `/scripts/build`, `/jobs/test/steps/0` |
| CSS | A rule's selector: `.card > .title` |
| Markdown, Mermaid | A unit's name: `docs/auth.md#refresh` |

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
  declaration by itself, an anchor with the one space before it. So adding,
  removing, or updating anchors and inner units never changes it, while any
  other edit inside the unit does. The only mode for document units.

A digest is the first 16 hex digits of SHA-256, and check recomputes it with
the mode recorded in the anchor.

## Commands

### check

```sh
dinyd check [--index | --worktree | --rev <commit>] [--changed [--base <rev>]]
```

Judges every anchor and prints one line per failure; exit 1 if any fail.

```
docs/auth.md → src/Hasher.cs#PasswordHasher.Verify: stale — re-judge the claim, then update
```

| Verdict | Meaning | Next step |
|---|---|---|
| `stale` | The target changed since the digest | Re-judge the claim, then `update` |
| `undeclared` | A hand-written digest anchor without a digest | `update` fills it |
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
dinyd apply <doc> --line <n> --selector <selector> [--bytes | --ref] [--index | --worktree | --rev <commit>]
```

Writes one anchor with its mode and first digest: at the end of line `n` in
Markdown, on a new `%%` line after line `n` in Mermaid. It prints the
anchor and names the view the target was read from. It refuses, writing
nothing, when the target does not resolve, the same anchor is already there,
or check's parser would not read the written anchor, as inside a code block
or Mermaid frontmatter.

### update

```sh
dinyd update <doc> --line <n> --expect-selector <selector> [--index | --worktree | --rev <commit>]
```

Recomputes the digest of the one digest anchor on line `n` whose selector is
`<selector>`, with its recorded mode, and rewrites only the digest. It prints
the new `mode:digest`, or `unchanged`. To change the mode, edit it in the
anchor by hand and run update.

Line numbers count from 1 by LF. apply and update write only the anchor and
keep every other byte, CRLF included. After either, stage the document before
judging with `--index`.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | check: every anchor is ok; apply, update: written or unchanged |
| 1 | check: at least one failure; apply, update: refused, nothing written |
| 2 | Usage or read error |

## License

MIT OR Apache-2.0
