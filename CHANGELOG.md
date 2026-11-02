# Changelog

User-facing changes and upgrade notes for released versions of DinyD.

For a project with `.dinyd-version`, `dinyd init --version <version>` selects the release and regenerates its selected host bindings. `dinyd update` upgrades the installed executable while preserving the project pin.

## 0.5.0

### Added

- Rust symbol selectors for functions, types, inline modules, inherent and trait methods, fields, enum variants, constants, statics, associated items, and macro definitions. Examples include `src/lib.rs#module.Type.method` and `src/lib.rs#Type.Trait.method`.
- Rust evidence includes the outer attributes immediately preceding an item, so changes to attributes such as `#[cfg(...)]` and `#[derive(...)]` change its digest. Trait implementations use the last type and trait name segments, without generics or references.
- New Rust anchors default to structural `s1` digests. Existing anchors keep their recorded digest mode.

### Changed

- `dinyd apply` now binds to the first **unbound** matching link whose usage ends on the requested line and names the selector's repository path, even when an earlier link to the same file already has an anchor. It writes a standalone anchor when no matching unbound link remains.

### Upgrade notes

- For a pinned project, select this release with [`dinyd init --version 0.5.0`](https://github.com/undervoke/dinyd/blob/v0.5.0/README.md#project-setup-and-versions) to use the Rust selectors and new link placement.

[Full changes](https://github.com/undervoke/dinyd/compare/v0.4.0...v0.5.0) · [Version documentation](https://github.com/undervoke/dinyd/blob/v0.5.0/README.md)

## 0.4.0

### Added

- Digest anchors can inherit the path of the Markdown link or image immediately before them. The comment supplies only a `#location`, or just a digest for the whole file. Only spaces or tabs may separate the link and anchor; reference-style links bind at their usage.
- `dinyd apply` now automatically writes a bound anchor immediately after the first matching link whose usage ends on the requested line, when that link is unbound. With no match, or when that first match is already bound, it appends a standalone anchor at the end of the line. `check` output and `digest --expect-selector` use the resolved repository path and location for bound anchors.

### Breaking changes and upgrade notes

- For a pinned project, select this release with [`dinyd init --version 0.4.0`](https://github.com/undervoke/dinyd/blob/v0.4.0/README.md#project-setup-and-versions) to adopt these changes.
- Removed `dinyd-ref` delegation anchors and `dinyd apply --ref`. Existing `dinyd-ref` comments become ordinary comments and are no longer checked.
- In Markdown documents, replace file or directory delegation references with ordinary Markdown links, which already receive path-existence checks.
- Use digest anchors for symbol or document-unit evidence; these also detect changes to the region. Mermaid links are not checked: use digest anchors there for file, symbol, or unit evidence, or keep existence-only path references in a Markdown document.
- Retained `dinyd-ref` comments inside document units now contribute to their `d1` digest instead of being excluded. After replacing these references, re-judge affected claims and refresh any stale anchors with `dinyd digest`.

[Full changes](https://github.com/undervoke/dinyd/compare/v0.3.1...v0.4.0) · [Version documentation](https://github.com/undervoke/dinyd/blob/v0.4.0/README.md)

## 0.3.1

### Fixed

- The OpenCode v2 guard now uses the session's absolute `location.directory` directly. Appending the project-relative `subpath` again built an incorrect nested path: a missing nested directory caused valid edits to be denied, while an existing duplicate directory could make the guard miss the real document's locks.

### Upgrade notes

- For projects using the OpenCode binding, run [`dinyd init --version 0.3.1`](https://github.com/undervoke/dinyd/blob/v0.3.1/README.md#project-setup-and-versions) to regenerate the managed adapter, then complete the host's reload steps.

[Full changes](https://github.com/undervoke/dinyd/compare/v0.3.0...v0.3.1) · [Version documentation](https://github.com/undervoke/dinyd/blob/v0.3.1/README.md)

## 0.3.0

### Added

- `dinyd init` prepares a project runtime, records an exact version in `.dinyd-version`, and configures selected agent-host bindings. Ordinary setup preserves an existing pin; `init --version <version>` explicitly changes it.
- Ordinary commands (`check`, `apply`, and `digest`) select the pinned runtime automatically and fetch it on a cache miss. Runtime downloads and self-update verify the archive's full SHA-256 before extraction, and the cache distinguishes versions and platforms.
- `dinyd update` installs the latest stable release over the invoked installation while preserving project pins.
- Document locks using `dinyd-lock` markers protect human-owned spans through agent file-edit hooks. Bindings support Claude Code, Codex Desktop, OpenCode v1 >= 1.18.29 and v2, Cursor Agent, Pi, and Kilo Code.
- Official shell and PowerShell bootstrap installers are attached to releases, including version-specific download URLs. Manual archives and checksums remain available because bootstrap installers do not guarantee checksum verification on every platform.

### Breaking changes and upgrade notes

- The document-evidence command `dinyd update <doc> --line <n> --expect-selector <selector>` is now `dinyd digest <doc> --line <n> --expect-selector <selector>`. Update scripts and instructions that use the old command; bare `dinyd update` now upgrades the installed executable.
- Commit `.dinyd-version` and the generated project settings. A fresh clone can run `dinyd init` to prepare the pinned runtime. A guard with a missing runtime requests initialization instead of downloading during a proposed edit.
- Init configures agent bindings; the consuming project still connects `check` to Git hooks or CI. The guard covers supported file-edit tools, not shell writes or Cursor Tab completions.

[Full changes](https://github.com/undervoke/dinyd/compare/v0.2.0...v0.3.0) · [Version documentation](https://github.com/undervoke/dinyd/blob/v0.3.0/README.md)

## 0.2.0

### Added

- Named document units delimited by `dinyd-id` markers can be referenced as `docs/guide.md#unit`. Their `d1` digest ignores DinyD metadata, so maintaining anchors does not invalidate the unit's evidence.
- Markdown links, images, and own-line Markdown Preview Enhanced `@import` directives are checked for local path existence without requiring an anchor. Destinations resolve from the document's directory; a leading `/` starts at the repository root. Queries and fragments are ignored for Markdown links and images, while an `@import` path is taken literally.
- `.mmd` files are recognized as Mermaid documents alongside `.mermaid` files, with own-line `%%` anchors and unit boundaries.

### Upgrade notes

- Broken local links that previously passed without an anchor now make `dinyd check` fail. Repair the destinations reported by the check. Remote URLs and same-document fragment links are outside these path checks.

[Full changes](https://github.com/undervoke/dinyd/compare/v0.1.0...v0.2.0) · [Version documentation](https://github.com/undervoke/dinyd/blob/v0.2.0/README.md)

## 0.1.0

Initial release: tie documentation claims to the source regions they restate and report when the recorded evidence changes.

### Included

- `dinyd check`, `dinyd apply`, and the original `dinyd update` command for checking, creating, and refreshing digest anchors in tracked Markdown and Mermaid documents. `dinyd-ref` provides existence-only references in this release.
- Symbol selectors for C#, TypeScript, JavaScript, Go, and Vue script blocks; JSON Pointer selectors for JSON and YAML; CSS rule selectors; and whole-file targets.
- Structural `s1` digests ignore whitespace between tokens while retaining comments and string content. Byte-based `b1` digests support other files and whole Vue files, with an explicit `apply --bytes` option.
- Consistent index, working-tree, and commit views through `--index` (the default), `--worktree`, and `--rev`, plus `--changed` checks against `HEAD` or an explicit base.
- Findings distinguish stale, undeclared, unresolved, and malformed anchors without printing document text or stored digests.
- Prebuilt archives and SHA-256 checksums for Linux x64/arm64, macOS x64/arm64, and Windows x64, plus installation from a Git tag with Cargo.

[Version documentation](https://github.com/undervoke/dinyd/blob/v0.1.0/README.md)
