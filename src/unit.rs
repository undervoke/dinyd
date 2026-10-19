//! Document units: the span of a .md or .mermaid document between `dinyd-id: NAME` and
//! `/dinyd-id: NAME`, with dinyd comments projected out.

use std::ops::Range;

use crate::anchor::{self, Comment, DocKind, Keyword};

/// The unit's bytes for d1, or why `name` does not name exactly one unit.
pub fn content(kind: DocKind, src: &str, name: &str) -> Result<Vec<u8>, String> {
    let comments = anchor::comments(kind, src);
    let (mut starts, mut ends) = (Vec::new(), Vec::new());
    for comment in &comments {
        match comment.declaration(src) {
            Some((false, n)) if n == name => starts.push(comment),
            Some((true, n)) if n == name => ends.push(comment),
            _ => {}
        }
    }
    let (start, end) = match (starts.as_slice(), ends.as_slice()) {
        ([], []) => return Err(format!("no unit {name}")),
        ([start], [end]) if end.extent.start >= start.extent.end => (start, end),
        ([_], [_]) => return Err(format!("unit {name} ends before it starts")),
        ([], _) => return Err(format!("unit {name} has no start")),
        (_, []) => return Err(format!("unit {name} has no end")),
        _ => return Err(format!("unit {name} is declared more than once")),
    };
    // An own-line boundary drops its whole line; an inline one stops at the comment itself.
    let from = own_line(src, &start.extent).map_or(start.extent.end, |line| line.end);
    let to = own_line(src, &end.extent).map_or(end.extent.start, |line| line.start);
    let region = from..to.max(from);

    let mut cuts: Vec<Range<usize>> = comments
        .iter()
        .filter(|c| c.terminated && c.extent.start >= region.start && c.extent.end <= region.end)
        .filter_map(|c| cut(src, c))
        .map(|r| r.start.max(region.start)..r.end.min(region.end))
        .collect();
    cuts.sort_by_key(|r| r.start);
    let mut out = Vec::with_capacity(region.len());
    let mut at = region.start;
    for cut in cuts {
        if cut.start > at {
            out.extend_from_slice(&src.as_bytes()[at..cut.start]);
        }
        at = at.max(cut.end);
    }
    out.extend_from_slice(&src.as_bytes()[at..region.end]);
    Ok(out)
}

/// What a dinyd comment inside a unit removes: its whole line when it stands alone there;
/// otherwise a declaration is zero-width and a reference takes the one space apply puts
/// before it.
fn cut(src: &str, comment: &Comment) -> Option<Range<usize>> {
    let (keyword, _) = comment.keyword(src)?;
    let extent = comment.extent.clone();
    Some(match own_line(src, &extent) {
        Some(line) => line,
        None if matches!(keyword, Keyword::Digest | Keyword::Ref)
            && src[..extent.start].ends_with(' ') =>
        {
            extent.start - 1..extent.end
        }
        None => extent,
    })
}

/// The comment's whole line, terminator included, when nothing but whitespace shares it.
fn own_line(src: &str, extent: &Range<usize>) -> Option<Range<usize>> {
    let start = src[..extent.start].rfind('\n').map_or(0, |i| i + 1);
    let end = src[extent.end..]
        .find('\n')
        .map_or(src.len(), |i| extent.end + i + 1);
    let blank = |s: &str| s.bytes().all(|b| b.is_ascii_whitespace());
    (blank(&src[start..extent.start]) && blank(&src[extent.end..end])).then_some(start..end)
}
