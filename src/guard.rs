//! Pre-execution guard: refuse file-editing tool calls that would change locked source.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::anchor::DocKind;
use crate::lock;

/// Host slugs the guard answers for. Orca uses the binding of the agent it launches.
pub const HOSTS: [&str; 6] = ["claude-code", "codex", "cursor", "opencode", "pi", "kilo"];

/// Whether the slug names a supported host.
pub fn supports(host: &str) -> bool {
    HOSTS.contains(&host)
}

fn is_plugin(host: &str) -> bool {
    matches!(host, "opencode" | "pi" | "kilo")
}

/// Project directory for one call: the host's explicit session context wins over the
/// process directory, since a hook may run in the main checkout while the agent edits
/// another worktree.
pub fn context(payload: &Value, fallback: &Path) -> PathBuf {
    if let Some(dir) = dir_field(payload, "worktree") {
        return dir;
    }
    if let Some(dir) = dir_field(payload, "cwd") {
        return dir;
    }
    if let Some(nested) = payload.get("context")
        && let Some(dir) = dir_field(nested, "worktree").or_else(|| dir_field(nested, "cwd"))
    {
        return dir;
    }
    if let Some(roots) = payload.get("workspace_roots").and_then(Value::as_array) {
        for root in roots {
            if let Some(dir) = dir_value(root) {
                return dir;
            }
        }
    }
    fallback.to_path_buf()
}

fn dir_field(payload: &Value, key: &str) -> Option<PathBuf> {
    payload.get(key).and_then(dir_value)
}

fn dir_value(value: &Value) -> Option<PathBuf> {
    let dir = value.as_str()?;
    (!dir.is_empty()).then(|| PathBuf::from(dir))
}

/// Judge one call against the directory from [`context`]; the first failing file denies
/// it all. The reason carries only a path and a short instruction.
pub fn evaluate(host: &str, payload: &Value, cwd: &Path) -> Result<(), String> {
    let Some(call) = tool_call(payload) else {
        return Ok(());
    };
    for op in operations(&call.tool, &call.args, host) {
        check_op(cwd, &op)?;
    }
    Ok(())
}

/// Decision in the host's own shape. It travels in stdout JSON, so the exit stays 0.
pub fn response(host: &str, reason: Option<&str>) -> (Value, u8) {
    match (is_plugin(host), reason) {
        (true, Some(reason)) => (json!({"block": true, "reason": reason}), 0),
        (true, None) => (json!({"block": false}), 0),
        (false, Some(reason)) if host == "cursor" => {
            (json!({"permission": "deny", "user_message": reason}), 0)
        }
        (false, Some(reason)) => (
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }}),
            0,
        ),
        (false, None) => (json!({}), 0),
    }
}

struct ToolCall {
    tool: String,
    args: Value,
}

/// One native call or one plugin `{tool, args}` envelope. Anything else is not a
/// file-editing tool call and is allowed.
fn tool_call(payload: &Value) -> Option<ToolCall> {
    if let Some(tool) = payload.get("tool").and_then(Value::as_str) {
        return Some(ToolCall {
            tool: tool.to_string(),
            args: payload.get("args").cloned().unwrap_or(Value::Null),
        });
    }
    if let Some(tool) = payload.get("tool_name").and_then(Value::as_str) {
        return Some(ToolCall {
            tool: tool.to_string(),
            args: payload.get("tool_input").cloned().unwrap_or(Value::Null),
        });
    }
    None
}

/// One exact-text replacement; `all` repeats it for every occurrence.
/// Pi matches every edit against the original file simultaneously and never
/// sends this flag; other hosts apply edits one after another.
struct TextEdit {
    old: String,
    new: String,
    all: bool,
}

enum Op {
    Overwrite {
        path: String,
        content: String,
    },
    Add {
        path: String,
        content: String,
    },
    Replace {
        path: String,
        edits: Vec<TextEdit>,
        simultaneous: bool,
    },
    PatchHunks {
        path: String,
        chunks: Vec<PatchChunk>,
    },
    MovePatch {
        from: String,
        to: String,
        chunks: Vec<PatchChunk>,
    },
    Delete {
        path: String,
    },
    Unknown {
        path: String,
    },
}

fn operations(tool: &str, args: &Value, host: &str) -> Vec<Op> {
    let name: String = tool
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    match name.as_str() {
        "edit" | "multiedit" => match path_field(args) {
            Some(path) => vec![replace_op(path, args, host == "pi")],
            None => vec![],
        },
        "write" => match path_field(args) {
            Some(path) => match full_content(args) {
                Some(content) => vec![Op::Overwrite { path, content }],
                None => vec![Op::Unknown { path }],
            },
            None => vec![],
        },
        "applypatch" => match patch_text(args) {
            Some(text) => expand_patch(&text),
            None => match path_field(args) {
                Some(path) => vec![Op::Unknown { path }],
                None => vec![],
            },
        },
        "delete" => match path_field(args) {
            Some(path) => vec![Op::Delete { path }],
            None => vec![],
        },
        _ => vec![],
    }
}

/// One replacement: a top-level old/new pair, or an `edits` array of pairs as Pi
/// sends them. A top-level replace flag covers every edit; each item may also
/// carry its own. Neither shape means the call cannot be previewed.
fn replace_op(path: String, args: &Value, simultaneous: bool) -> Op {
    let top = replace_all(args);
    if let (Some(old), Some(new)) = (old_of(args), new_of(args)) {
        return Op::Replace {
            path,
            edits: vec![TextEdit { old, new, all: top }],
            simultaneous,
        };
    }
    match edit_pairs(args, top) {
        Some(edits) => Op::Replace {
            path,
            edits,
            simultaneous,
        },
        None => Op::Unknown { path },
    }
}

fn str_key(args: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| args.get(key).and_then(Value::as_str))
        .map(str::to_string)
}

fn path_field(args: &Value) -> Option<String> {
    str_key(args, &["file_path", "filePath", "path"])
}

fn full_content(args: &Value) -> Option<String> {
    str_key(args, &["content", "text"])
}

fn old_of(args: &Value) -> Option<String> {
    str_key(args, &["old_string", "oldString", "old_text", "oldText"])
}

fn new_of(args: &Value) -> Option<String> {
    str_key(args, &["new_string", "newString", "new_text", "newText"])
}

/// Codex sends the patch as `command`, OpenCode and Kilo adapters as `patchText`.
fn patch_text(args: &Value) -> Option<String> {
    str_key(args, &["patch", "patchText", "command"])
}

fn replace_all(args: &Value) -> bool {
    args.get("replace_all")
        .or_else(|| args.get("replaceAll"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn edit_pairs(args: &Value, top: bool) -> Option<Vec<TextEdit>> {
    let items = args.get("edits")?.as_array()?;
    if items.is_empty() {
        return None;
    }
    items
        .iter()
        .map(|item| {
            old_of(item).zip(new_of(item)).map(|(old, new)| TextEdit {
                old,
                new,
                all: top || replace_all(item),
            })
        })
        .collect()
}

enum Current {
    Missing,
    Unreadable,
    Binary,
    Text(String),
}

fn resolve(cwd: &Path, given: &str) -> PathBuf {
    let path = Path::new(given);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn current(cwd: &Path, given: &str) -> Current {
    let bytes = match std::fs::read(resolve(cwd, given)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Current::Missing,
        Err(_) => return Current::Unreadable,
    };
    match String::from_utf8(bytes) {
        Ok(text) => Current::Text(text),
        Err(_) => Current::Binary,
    }
}

/// Locked state needing a preview, or `None` when none is needed: a missing file,
/// non-text bytes, another file kind, or no lock markers.
fn locked_current(
    cwd: &Path,
    given: &str,
) -> Result<Option<(DocKind, String, lock::Parsed)>, String> {
    match current(cwd, given) {
        Current::Missing => Ok(None),
        Current::Unreadable => Err(lock::reread_denial(given)),
        Current::Binary => Ok(None),
        Current::Text(text) => {
            let Some(kind) = DocKind::of(given) else {
                return Ok(None);
            };
            let pre = lock::parse(kind, &text);
            if pre.recognized() && !pre.sound() {
                return Err(lock::malformed_denial(given));
            }
            if pre.recognized() {
                Ok(Some((kind, text, pre)))
            } else {
                Ok(None)
            }
        }
    }
}

fn check_op(cwd: &Path, op: &Op) -> Result<(), String> {
    match op {
        Op::Overwrite { path, content } => {
            if let Some((kind, text, pre)) = locked_current(cwd, path)? {
                check_preview(path, kind, &text, &pre, content)?;
            }
            Ok(())
        }
        Op::Add { path, content } => {
            if matches!(current(cwd, path), Current::Text(_))
                && let Some((kind, text, pre)) = locked_current(cwd, path)?
            {
                check_preview(path, kind, &text, &pre, content)?;
            }
            Ok(())
        }
        Op::Replace {
            path,
            edits,
            simultaneous,
        } => {
            let Some((kind, text, pre)) = locked_current(cwd, path)? else {
                return Ok(());
            };
            let preview = if *simultaneous {
                apply_simultaneous(&text, edits)
            } else {
                let mut preview = text.clone();
                let mut failed = false;
                for edit in edits {
                    let next = if edit.all {
                        apply_replace_all(&preview, &edit.old, &edit.new)
                    } else {
                        apply_replace(&preview, &edit.old, &edit.new)
                    };
                    match next {
                        Some(next) => preview = next,
                        None => {
                            failed = true;
                            break;
                        }
                    }
                }
                (!failed).then_some(preview)
            };
            let preview = preview.ok_or_else(|| lock::reread_denial(path))?;
            check_preview(path, kind, &text, &pre, &preview)?;
            Ok(())
        }
        Op::PatchHunks { path, chunks } => {
            let Some((kind, text, pre)) = locked_current(cwd, path)? else {
                return Ok(());
            };
            let preview = apply_hunks(&text, chunks).ok_or_else(|| lock::reread_denial(path))?;
            check_preview(path, kind, &text, &pre, &preview)?;
            Ok(())
        }
        Op::MovePatch { from, to, chunks } => {
            let source = move_source(cwd, from)?;
            check_move_dest(cwd, to, source.as_deref(), |text| apply_hunks(text, chunks))
        }
        Op::Delete { path } => {
            if locked_current(cwd, path)?.is_some() {
                return Err(lock::moved_denial(path));
            }
            Ok(())
        }
        Op::Unknown { path } => {
            if locked_current(cwd, path)?.is_some() {
                return Err(lock::reread_denial(path));
            }
            Ok(())
        }
    }
}

/// Locked move sources deny outright; anything else yields the bytes for the
/// destination preview, or `None` when there is nothing to carry over.
fn move_source(cwd: &Path, from: &str) -> Result<Option<String>, String> {
    match current(cwd, from) {
        Current::Missing | Current::Binary => Ok(None),
        Current::Unreadable => Err(lock::reread_denial(from)),
        Current::Text(text) => {
            if let Some(kind) = DocKind::of(from) {
                let pre = lock::parse(kind, &text);
                if pre.recognized() {
                    if !pre.sound() {
                        return Err(lock::malformed_denial(from));
                    }
                    return Err(lock::moved_denial(from));
                }
            }
            Ok(Some(text))
        }
    }
}

/// A move lands source bytes on its destination, so a locked destination previews
/// like an overwrite. Without source bytes a locked destination cannot be previewed.
fn check_move_dest(
    cwd: &Path,
    to: &str,
    source: Option<&str>,
    preview: impl FnOnce(&str) -> Option<String>,
) -> Result<(), String> {
    match current(cwd, to) {
        Current::Missing | Current::Binary => Ok(()),
        Current::Unreadable => Err(lock::reread_denial(to)),
        Current::Text(text) => {
            let Some(kind) = DocKind::of(to) else {
                return Ok(());
            };
            let pre = lock::parse(kind, &text);
            if !pre.recognized() {
                return Ok(());
            }
            if !pre.sound() {
                return Err(lock::malformed_denial(to));
            }
            match source.and_then(preview) {
                Some(next) => check_preview(to, kind, &text, &pre, &next),
                None => Err(lock::reread_denial(to)),
            }
        }
    }
}

/// Preview the re-parsed file: malformed post-edit markers deny, and every pre-edit
/// outermost span must survive as a recognized span. Lock-free files need no preview,
/// so a new file may introduce locks freely; this only runs for locked ones.
fn check_preview(
    given: &str,
    kind: DocKind,
    current: &str,
    pre: &lock::Parsed,
    new_src: &str,
) -> Result<(), String> {
    let post = lock::parse(kind, new_src);
    if !post.sound() {
        return Err(lock::malformed_denial(given));
    }
    if !lock::preserves(current, pre, new_src, &post) {
        return Err(lock::locked_denial(given));
    }
    Ok(())
}

/// One exact, unambiguous match. An empty, missing, or repeated `old` is not
/// reconstructible, so the guard denies rather than guessing.
fn apply_replace(src: &str, old: &str, new: &str) -> Option<String> {
    if old.is_empty() {
        return None;
    }
    let mut hits = src.match_indices(old);
    let (first, _) = hits.next()?;
    if hits.next().is_some() {
        return None;
    }
    let mut out = String::with_capacity(src.len());
    out.push_str(&src[..first]);
    out.push_str(new);
    out.push_str(&src[first + old.len()..]);
    Some(out)
}

/// Every occurrence, of which there must be at least one.
fn apply_replace_all(src: &str, old: &str, new: &str) -> Option<String> {
    if old.is_empty() || !src.contains(old) {
        return None;
    }
    Some(src.replace(old, new))
}

/// Pi matches every edit against the original file at once: each old text must
/// occur exactly once, with no overlaps, or the tool changes nothing. Endings
/// are normalized to LF for matching and restored after, as the tool does, and
/// only exact matches count. Uniqueness makes the `all` flag moot: a repeated
/// old text denies rather than guessing.
fn apply_simultaneous(src: &str, edits: &[TextEdit]) -> Option<String> {
    if edits.is_empty() || edits.iter().any(|edit| edit.old.is_empty()) {
        return None;
    }
    let ending = detect_ending(src);
    let normal = normalize_lf(src);
    let edits: Vec<(String, String)> = edits
        .iter()
        .map(|edit| (normalize_lf(&edit.old), normalize_lf(&edit.new)))
        .collect();
    let mut hits: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for (old, new) in &edits {
        let mut matches = normal.match_indices(old);
        let (at, _) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        hits.push((at, old.len(), new.as_str()));
    }
    hits.sort_by_key(|&(at, _, _)| at);
    let mut out = String::with_capacity(normal.len());
    let mut cursor = 0;
    for (at, len, new) in hits {
        if at < cursor {
            return None;
        }
        out.push_str(&normal[cursor..at]);
        out.push_str(new);
        cursor = at + len;
    }
    out.push_str(&normal[cursor..]);
    Some(restore_ending(&out, ending))
}

/// The file's line ending for a simultaneous preview: CRLF only when the first
/// line break is CRLF, as the tool detects it.
fn detect_ending(src: &str) -> &'static str {
    match (src.find('\n'), src.find("\r\n")) {
        (None, _) | (_, None) => "\n",
        (Some(lf), Some(crlf)) if crlf < lf => "\r\n",
        _ => "\n",
    }
}

fn normalize_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_ending(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// One hunk of an `Update File` patch: the block to replace plus the context that
/// pins it. `eof` is set by a trailing `*** End of File`.
struct PatchChunk {
    change_context: Option<String>,
    old: Vec<String>,
    new: Vec<String>,
    /// Indices into `old` and `new` parsed as context lines, kept byte-for-byte
    /// from the original rather than rewritten.
    context: Vec<(usize, usize)>,
    eof: bool,
}

impl PatchChunk {
    /// A context line belongs to both sides while staying distinguishable from
    /// identical changed lines.
    fn push_context(&mut self, line: String) {
        self.context.push((self.old.len(), self.new.len()));
        self.old.push(line.clone());
        self.new.push(line);
    }

    fn is_empty(&self) -> bool {
        self.old.is_empty() && self.new.is_empty()
    }
}

/// One `Update File` section while the patch is still being read.
struct FileUpdate {
    path: String,
    move_to: Option<String>,
    chunks: Vec<PatchChunk>,
    eof_closed: bool,
}

impl FileUpdate {
    fn chunk_mut(&mut self) -> &mut PatchChunk {
        if self.chunks.is_empty() {
            self.chunks.push(PatchChunk {
                change_context: None,
                old: Vec::new(),
                new: Vec::new(),
                context: Vec::new(),
                eof: false,
            });
        }
        self.chunks.last_mut().unwrap()
    }
}

enum PatchEntry {
    Update(FileUpdate),
    Add { path: String, content: String },
    Delete { path: String },
}

/// The patch framing the tool enforces: `*** Begin Patch` first, `*** End Patch`
/// last, with the documented heredoc unwrapping. Anything else is rejected
/// outright, so there is nothing to preview.
fn patch_body(text: &str) -> Option<Vec<String>> {
    let lines: Vec<String> = text
        .trim()
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if let [first, .., last] = lines.as_slice()
        && (*first == "<<EOF" || *first == "<<'EOF'" || *first == "<<\"EOF\"")
        && last.ends_with("EOF")
        && lines.len() >= 4
    {
        return patch_body(&lines[1..lines.len() - 1].join("\n"));
    }
    match (lines.first(), lines.last()) {
        (Some(first), Some(last))
            if first.trim() == "*** Begin Patch" && last.trim() == "*** End Patch" =>
        {
            Some(lines)
        }
        _ => None,
    }
}

/// Split one `apply_patch` text into per-file operations, mirroring the tool's
/// parser and matcher without its fuzzy fallbacks: `*** Move to:` belongs to the
/// open update, `@@ <line>` seeks past that line, `*** End of File` pins the last
/// chunk to the file end. Malformed input fails the exact parse, and only the
/// explicit Update/Add/Delete/Move-to header paths recover as unverifiable
/// operations, so known locked files deny; there is no guessing beyond those
/// four header forms. Header paths stay literal: the tool does not unquote.
fn expand_patch(text: &str) -> Vec<Op> {
    match parse_patch(text) {
        Ok(entries) => entries.into_iter().map(entry_op).collect(),
        Err(()) => explicit_paths(text)
            .into_iter()
            .map(|path| Op::Unknown { path })
            .collect(),
    }
}

/// Explicit file paths named by patch headers. On a malformed patch the exact
/// parse fails, but these paths are still the files the patch claims to touch,
/// so they preview as unverifiable rather than silently passing. No guessing
/// beyond the four explicit header forms.
fn explicit_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for raw in text.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw).trim();
        for marker in [
            "*** Update File: ",
            "*** Add File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(path) = line.strip_prefix(marker)
                && !path.is_empty()
                && !paths.contains(&path.to_string())
            {
                paths.push(path.to_string());
                break;
            }
        }
    }
    paths
}

fn parse_patch(text: &str) -> Result<Vec<PatchEntry>, ()> {
    let Some(lines) = patch_body(text) else {
        return Err(());
    };
    let mut entries: Vec<PatchEntry> = Vec::new();
    let mut update: Option<FileUpdate> = None;
    let mut adding: Option<(String, Vec<String>)> = None;
    // Close the open update, dropping a trailing chunk that carries no lines.
    // An empty path or no chunks at all is rejected by the tool.
    let close_update = |update: &mut Option<FileUpdate>, entries: &mut Vec<PatchEntry>| -> bool {
        if let Some(mut update) = update.take() {
            if update.chunks.last().is_some_and(PatchChunk::is_empty) {
                update.chunks.pop();
            }
            if update.path.is_empty() || update.chunks.is_empty() {
                return false;
            }
            entries.push(PatchEntry::Update(update));
        }
        true
    };
    for raw in lines {
        // The tool strips one trailing carriage return per line before parsing.
        let line: &str = raw.strip_suffix('\r').unwrap_or(&raw);
        // Inside an update only the right edge is trimmed, so an indented `***`
        // is content there; outside, whole-line headers open a new section.
        let header = if update.is_some() {
            line.trim_end()
        } else {
            line.trim()
        };
        if header.starts_with("***") {
            // `*** Move to:` and `*** End of File` belong to the open update.
            if let Some(update) = update.as_mut() {
                if let Some(dest) = header.strip_prefix("*** Move to: ") {
                    if update.chunks.is_empty() && update.move_to.is_none() {
                        update.move_to = Some(dest.to_string());
                        continue;
                    }
                    return Err(());
                }
                if header == "*** End of File" {
                    match update.chunks.last_mut() {
                        Some(last) if !last.is_empty() => {
                            last.eof = true;
                            update.eof_closed = true;
                        }
                        Some(_) => return Err(()),
                        None => {}
                    }
                    continue;
                }
            }
            if !close_update(&mut update, &mut entries) {
                return Err(());
            }
            if let Some((path, buf)) = adding.take() {
                entries.push(PatchEntry::Add {
                    path,
                    content: finish_add(buf),
                });
            }
            if let Some(path) = header.strip_prefix("*** Update File: ") {
                update = Some(FileUpdate {
                    path: path.to_string(),
                    move_to: None,
                    chunks: Vec::new(),
                    eof_closed: false,
                });
            } else if let Some(path) = header.strip_prefix("*** Add File: ") {
                adding = Some((path.to_string(), Vec::new()));
            } else if let Some(path) = header.strip_prefix("*** Delete File: ") {
                entries.push(PatchEntry::Delete {
                    path: path.to_string(),
                });
            } else if header == "*** Begin Patch"
                || header == "*** End Patch"
                || update.is_none() && header.starts_with("*** Environment ID:")
            {
                // Framing only.
            } else {
                return Err(());
            }
            continue;
        }
        if let Some(update) = update.as_mut() {
            // After `*** End of File` only empty lines and new chunks continue.
            if update.eof_closed && !line.is_empty() && header != "@@" && !header.starts_with("@@ ")
            {
                return Err(());
            }
            if header == "@@" || header.starts_with("@@ ") {
                if update.chunks.last().is_some_and(PatchChunk::is_empty) {
                    // Back-to-back chunk headers are rejected by the tool.
                    return Err(());
                }
                update.eof_closed = false;
                update.chunks.push(PatchChunk {
                    change_context: header.strip_prefix("@@ ").map(str::to_string),
                    old: Vec::new(),
                    new: Vec::new(),
                    context: Vec::new(),
                    eof: false,
                });
                continue;
            }
            if line.is_empty() {
                update.chunk_mut().push_context(String::new());
            } else if let Some(rest) = line.strip_prefix(' ') {
                update.chunk_mut().push_context(rest.to_string());
            } else if let Some(rest) = line.strip_prefix('+') {
                update.chunk_mut().new.push(rest.to_string());
            } else if let Some(rest) = line.strip_prefix('-') {
                update.chunk_mut().old.push(rest.to_string());
            } else {
                // `\` markers and anything else are rejected by the tool.
                return Err(());
            }
            continue;
        }
        if let Some((_, buf)) = adding.as_mut() {
            // Added lines carry one leading `+`; anything else is rejected.
            if let Some(rest) = line.strip_prefix('+') {
                buf.push(rest.to_string());
            } else {
                return Err(());
            }
            continue;
        }
        if !line.trim().is_empty() {
            // Outside a file section the tool accepts nothing but headers.
            return Err(());
        }
    }
    if !close_update(&mut update, &mut entries) {
        return Err(());
    }
    if let Some((path, buf)) = adding.take() {
        entries.push(PatchEntry::Add {
            path,
            content: finish_add(buf),
        });
    }
    Ok(entries)
}

/// Added lines joined with newlines and one trailing newline, as written.
fn finish_add(buf: Vec<String>) -> String {
    if buf.is_empty() {
        String::new()
    } else {
        buf.join("\n") + "\n"
    }
}

fn entry_op(entry: PatchEntry) -> Op {
    match entry {
        PatchEntry::Update(update) => match update.move_to {
            Some(to) => Op::MovePatch {
                from: update.path,
                to,
                chunks: update.chunks,
            },
            None => Op::PatchHunks {
                path: update.path,
                chunks: update.chunks,
            },
        },
        PatchEntry::Add { path, content } => Op::Add { path, content },
        PatchEntry::Delete { path } => Op::Delete { path },
    }
}

/// A planned file replacement: where it starts, how many lines it spans, and
/// the lines that take their place.
struct Replacement {
    start: usize,
    len: usize,
    new: Vec<String>,
}

/// Plan every chunk against the file's logical lines, mirroring the tool's
/// replacement computation with exact matching only: no trailing-whitespace or
/// Unicode leniency. A `@@ <line>` seeks past that line first, an empty `old`
/// appends at the end, and an end-of-file chunk must match at the exact file end.
fn plan_replacements(lines: &[String], chunks: &[PatchChunk]) -> Option<Vec<Replacement>> {
    let mut out = Vec::new();
    let mut line_index = 0;
    for chunk in chunks {
        if let Some(context) = &chunk.change_context {
            line_index = seek_exact(lines, std::slice::from_ref(context), line_index, false)? + 1;
        }
        if chunk.old.is_empty() {
            out.push(Replacement {
                start: lines.len(),
                len: 0,
                new: chunk.new.clone(),
            });
            continue;
        }
        let mut pattern: &[String] = &chunk.old;
        let mut new_slice: &[String] = &chunk.new;
        let mut found = seek_exact(lines, pattern, line_index, chunk.eof);
        // A trailing empty line stands for the final newline, which the logical
        // lines do not carry; retry without it on both sides.
        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_exact(lines, pattern, line_index, chunk.eof);
        }
        let start = found?;
        // Context lines stay byte-for-byte from the original; only the runs
        // between them are replaced.
        let mut old_start = 0;
        let mut new_start = 0;
        for &(old_at, new_at) in &chunk.context {
            if old_at >= pattern.len() || new_at >= new_slice.len() {
                break;
            }
            if old_start != old_at || new_start != new_at {
                out.push(Replacement {
                    start: start + old_start,
                    len: old_at - old_start,
                    new: new_slice[new_start..new_at].to_vec(),
                });
            }
            old_start = old_at + 1;
            new_start = new_at + 1;
        }
        if old_start != pattern.len() || new_start != new_slice.len() {
            out.push(Replacement {
                start: start + old_start,
                len: pattern.len() - old_start,
                new: new_slice[new_start..].to_vec(),
            });
        }
        line_index = start + pattern.len();
    }
    out.sort_by_key(|r| r.start);
    Some(out)
}

/// The tool's sequence search with exact matching only. With `eof`, only the
/// position ending exactly at the file end is tried.
fn seek_exact(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    if eof {
        let at = lines.len() - pattern.len();
        if at >= start && lines[at..at + pattern.len()] == *pattern {
            return Some(at);
        }
        return None;
    }
    (start..=lines.len() - pattern.len()).find(|&i| lines[i..i + pattern.len()] == *pattern)
}

#[derive(Clone, Copy)]
enum Ending {
    Lf,
    CrLf,
    Cr,
}

impl Ending {
    fn text(self) -> &'static str {
        match self {
            Ending::Lf => "\n",
            Ending::CrLf => "\r\n",
            Ending::Cr => "\r",
        }
    }
}

#[derive(Clone)]
struct SourceLine {
    text: String,
    ending: Option<Ending>,
}

/// The file as logical lines retaining each terminator, mirroring the tool's
/// source file: the first ending becomes the style for inserted lines, and a
/// missing trailing ending defaults to LF.
fn parse_source(contents: &str) -> (Vec<SourceLine>, Ending) {
    let mut lines = Vec::new();
    let mut preferred = None;
    let mut text = String::new();
    let mut chars = contents.chars().peekable();
    while let Some(c) = chars.next() {
        let ending = match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                Some(Ending::CrLf)
            }
            '\r' => Some(Ending::Cr),
            '\n' => Some(Ending::Lf),
            _ => {
                text.push(c);
                None
            }
        };
        if let Some(ending) = ending {
            preferred.get_or_insert(ending);
            lines.push(SourceLine {
                text: std::mem::take(&mut text),
                ending: Some(ending),
            });
        }
    }
    if !text.is_empty() {
        lines.push(SourceLine { text, ending: None });
    }
    (lines, preferred.unwrap_or(Ending::Lf))
}

/// Rebuild the file from ordered replacements: untouched lines keep their exact
/// bytes and terminators, inserted lines take the file's preferred ending, and
/// every line ends up terminated, as the tool has always done.
fn apply_plan(
    source: Vec<SourceLine>,
    preferred: Ending,
    mut plan: Vec<Replacement>,
) -> Option<String> {
    plan.sort_by_key(|r| r.start);
    let mut out: Vec<SourceLine> = Vec::new();
    let mut at = 0;
    for rep in &plan {
        if rep.start < at || rep.start + rep.len > source.len() {
            return None;
        }
        out.extend(source[at..rep.start].iter().cloned());
        out.extend(rep.new.iter().map(|text| SourceLine {
            text: text.clone(),
            ending: Some(preferred),
        }));
        at = rep.start + rep.len;
    }
    out.extend(source[at..].iter().cloned());
    for line in &mut out {
        line.ending.get_or_insert(preferred);
    }
    let mut contents = String::new();
    for line in &out {
        contents.push_str(&line.text);
        contents.push_str(line.ending.map_or("", Ending::text));
    }
    Some(contents)
}

/// Preview a patched file by planning every chunk against the logical lines and
/// rebuilding with original terminators intact. Context lines and untouched
/// regions keep their exact bytes, so a CRLF file stays CRLF outside edited
/// lines. Anything the plan cannot place exactly denies rather than guessing.
fn apply_hunks(original: &str, chunks: &[PatchChunk]) -> Option<String> {
    let (source, preferred) = parse_source(original);
    let texts: Vec<String> = source.iter().map(|line| line.text.clone()).collect();
    let plan = plan_replacements(&texts, chunks)?;
    apply_plan(source, preferred, plan)
}
