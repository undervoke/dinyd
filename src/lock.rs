//! Locked document source: marker scans and post-edit preservation.
//!
//! A lock reserves source for human edits. Each protected span is both boundary comments
//! and every byte between them. Protection follows the source within its file: an intact
//! span may move, while deleting it, overwriting it, or moving its file is denied.

use std::ops::Range;

use crate::anchor::{DocKind, comments};

/// Every recognized lock span of one parse, with the outermost ones the next parse must
/// still contain and whether the markers themselves refuse edits.
pub struct Parsed {
    spans: Vec<Range<usize>>,
    outers: Vec<Range<usize>>,
    recognized: bool,
    malformed: bool,
    unpaired: bool,
}

impl Parsed {
    /// Whether any lock marker was recognized, well-formed or not.
    pub fn recognized(&self) -> bool {
        self.recognized
    }

    /// Whether the markers pair up without malformed bodies or lost terminators.
    pub fn sound(&self) -> bool {
        !self.malformed && !self.unpaired
    }
}

/// Scan the locks of `src`, ignoring markers inside literal examples: Markdown code and
/// Mermaid frontmatter define no locks, as with anchors.
pub fn parse(kind: DocKind, src: &str) -> Parsed {
    let mut spans = Vec::new();
    let mut outers = Vec::new();
    let mut stack: Vec<(Range<usize>, usize)> = Vec::new();
    let (mut recognized, mut malformed, mut unpaired) = (false, false, false);
    for comment in comments(kind, src) {
        let Some((end, wellformed)) = comment.lock_marker(src) else {
            continue;
        };
        recognized = true;
        if !wellformed {
            malformed = true;
            continue;
        }
        if !end {
            let depth = stack.len();
            stack.push((comment.extent.clone(), depth));
        } else if let Some((start, depth)) = stack.pop() {
            let span = start.start..comment.extent.end;
            spans.push(span.clone());
            if depth == 0 {
                outers.push(span);
            }
        } else {
            unpaired = true;
        }
    }
    if !stack.is_empty() {
        unpaired = true;
    }
    spans.sort_by_key(|r| r.start);
    outers.sort_by_key(|r| r.start);
    Parsed {
        spans,
        outers,
        recognized,
        malformed,
        unpaired,
    }
}

/// Whether every pre-edit outermost span survives byte-for-byte, in relative order, as a
/// recognized post-edit span. Matching against all post spans, inner ones included, is
/// what permits new enclosing locks around untouched source. Re-parsing the post-edit
/// file matters: an unclosed code fence can deactivate a marker without changing any of
/// its bytes, and then no recognized span carries it.
pub fn preserves(old_src: &str, old: &Parsed, new_src: &str, new: &Parsed) -> bool {
    let mut at = 0;
    for outer in &old.outers {
        let want = &old_src[outer.clone()];
        let mut found = false;
        while at < new.spans.len() {
            if new_src[new.spans[at].clone()] == *want {
                found = true;
                at += 1;
                break;
            }
            at += 1;
        }
        if !found {
            return false;
        }
    }
    true
}

/// Control characters in a tool-supplied path are replaced so a hostile path cannot
/// inject extra lines into the agent-visible refusal.
fn safe(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

/// The refusal carries only the path and a short instruction, never document text.
pub fn locked_denial(path: &str) -> String {
    format!("{}: locked section. Leave it unchanged.", safe(path))
}

/// Malformed markers need a human: the guard cannot tell what the owner meant to protect.
pub fn malformed_denial(path: &str) -> String {
    format!(
        "{}: malformed dinyd-lock markers; a human must repair them.",
        safe(path)
    )
}

/// An edit the guard cannot reconstruct exactly is denied rather than guessed through.
pub fn reread_denial(path: &str) -> String {
    format!(
        "{}: cannot preview the edit; re-read the file and retry.",
        safe(path)
    )
}

/// A locked file stays where it is.
pub fn moved_denial(path: &str) -> String {
    format!(
        "{}: locked file cannot be moved or deleted. Leave it unchanged.",
        safe(path)
    )
}
