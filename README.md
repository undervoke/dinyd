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

With a Rust toolchain, `cargo install --locked --git https://github.com/undervoke/dinyd --tag v0.1.0`
builds the same version from source.

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

Documents are tracked `*.md` and `*.mermaid` files. In Markdown an anchor is
an HTML comment anywhere outside code blocks and inline code. In `.mermaid` it
is an own-line `%%` comment, after any leading frontmatter.

### Selectors

`path#location`. The path is the Git path from the repository root, with `/`
as the only separator. Without `#location` the selector is the whole file.

| Target | Location |
|---|---|
| C#, TypeScript, JavaScript, Go | Symbol path: `Class.Method`, `Namespace.Type.Field`, `Type.Method` (Go), `Enum.Member` |
| Vue | Symbol path inside the top-level `<script>` |
| JSON, YAML | JSON Pointer: `/scripts/build`, `/jobs/test/steps/0` |
| CSS | A rule's selector: `.card > .title` |

C# namespaces are not part of the path; TypeScript namespaces and modules
are. Members that share a name (overloads) are ambiguous; `Verify(*)` takes
all of them, in source order, as one region.

### Digest modes

- `s1` (structural): the target's tokens and nesting. Whitespace between
  tokens is ignored; comments count, and text inside strings and comments
  is kept exactly. The default for the code and data formats above.
- `b1` (bytes): the exact bytes with CRLF turned into LF. The default for
  other files and whole `.vue` files; `apply --bytes` chooses it anywhere.

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
| `unresolved` | The target is missing, ambiguous, or unreadable | Re-judge the claim and the reference |
| `malformed` | The anchor itself is broken | Re-judge the claim and the reference |

Output never carries line numbers, digests, or document text; the reason a
target did not resolve goes to stderr.

- `--index` (default) reads documents and targets from the index: the gate
  view. `--worktree` reads the disk: the view while working. `--rev` reads a
  commit. Documents and targets always come from the same view.
- `--changed` judges only anchors in changed documents and anchors whose
  target path changed, including deletions and renames, against `HEAD` or
  `--base`. With `--rev`, `--base` is required. Malformed anchors are always
  judged.

### apply

```sh
dinyd apply <doc> --line <n> --selector <selector> [--bytes | --ref] [--index | --worktree | --rev <commit>]
```

Writes one anchor with its mode and first digest: at the end of line `n` in
Markdown, on a new `%%` line after line `n` in `.mermaid`. It prints the
anchor and names the view the target was read from. It refuses, writing
nothing, when the target does not resolve, the same anchor is already there,
or the anchor would land where check cannot read it (inside code, a comment,
or frontmatter).

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
