//! apply and digest: each touches one anchor in one document.

use std::path::Path;

use crate::Error;
use crate::anchor::{self, Anchor, DocKind, LineIndex, Mode, Reference, Selector};
use crate::git::{Basis, Repo, Snapshot};
use crate::link::{self, References};
use crate::target::{self, Failure};

pub struct ApplyRequest<'a> {
    pub doc: &'a Path,
    pub line: usize,
    pub selector: &'a str,
    pub bytes: bool,
    pub basis: Basis,
}

/// Where apply writes the anchor.
enum Placement {
    /// Right after the link usage ending at this offset, inheriting its path.
    Bound(usize),
    /// At the end of the line, naming its path.
    Standalone,
}

/// Writes a new anchor and returns its text with the basis its digest came from.
pub fn apply(repo: &Repo, request: ApplyRequest) -> Result<(String, String), Error> {
    let (doc, kind, source) = read_doc(repo, request.doc)?;
    let lines = LineIndex::new(&source);
    check_line(&source, &lines, request.line)?;
    let references = link::references(&doc, kind, &source);
    let (placement, selector) = place(&references, &lines, request.line, request.selector)?;
    if request.bytes && selector.location.is_some() && DocKind::of(&selector.path).is_some() {
        return Err(Error::Usage(
            "--bytes does not apply to a document unit; units use d1".to_string(),
        ));
    }

    let snapshot = Snapshot::load(repo, request.basis)?;
    let basis = snapshot.basis().describe();
    let (mode, value) =
        match target::digest(&snapshot, &selector, request.bytes.then_some(Mode::B1)) {
            Ok(digest) => digest,
            Err(Failure::Unresolved(reason)) => {
                return Err(Error::Refused(format!(
                    "{}: unresolved in {basis} ({reason}); nothing written",
                    request.selector
                )));
            }
            Err(Failure::Read(e)) => return Err(e),
        };
    let named = match (&placement, &selector.location) {
        (Placement::Standalone, _) => Some(request.selector.to_string()),
        (Placement::Bound(_), location) => location.as_ref().map(|l| format!("#{l}")),
    };
    let text = match named {
        Some(named) => format!("dinyd: {named} {}:{value}", mode.as_str()),
        None => format!("dinyd: {}:{value}", mode.as_str()),
    };

    let target_line = match kind {
        DocKind::Markdown => request.line,
        DocKind::Mermaid => request.line + 1,
    };
    if references
        .anchors
        .iter()
        .any(|a| a.line == target_line && judges(a, &selector))
    {
        return Err(Error::Refused(format!(
            "an anchor for {} already exists at line {target_line}; nothing written",
            request.selector
        )));
    }

    let content = lines.content(&source, request.line);
    let mut updated = source.clone();
    match (placement, kind) {
        // No gap, so the rendered text around the link stays as it was.
        (Placement::Bound(end), _) => updated.insert_str(end, &format!("<!-- {text} -->")),
        (Placement::Standalone, DocKind::Markdown) => {
            let space = if content.is_empty() { "" } else { " " };
            updated.insert_str(content.end, &format!("{space}<!-- {text} -->"));
        }
        (Placement::Standalone, DocKind::Mermaid) => {
            let line_text = &source[content.clone()];
            let indent = &line_text[..line_text.len() - line_text.trim_start().len()];
            let end = lines.end(&source, request.line);
            if end > content.end {
                let eol = &source[content.end..end];
                updated.insert_str(end, &format!("{indent}%% {text}{eol}"));
            } else {
                let eol = if source.contains("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                updated.insert_str(end, &format!("{eol}{indent}%% {text}"));
            }
        }
    }

    let visible = link::references(&doc, kind, &updated)
        .anchors
        .iter()
        .any(|a| {
            a.line == target_line
                && judges(a, &selector)
                && a.digest
                    .as_ref()
                    .is_some_and(|(m, v)| *m == mode && *v == value)
        });
    if !visible {
        return Err(Error::Refused(format!(
            "line {} would not read the anchor back (as in code, a comment, or frontmatter); nothing written",
            request.line
        )));
    }
    write_doc(repo, &doc, &updated)?;
    Ok((text, basis))
}

/// Where the anchor goes and what it selects. The first unbound link usage ending on `line` whose
/// path the selector names takes the anchor, which then inherits that path; if every such usage
/// is already bound, the new one goes to the end of the line with its path written out.
fn place(
    references: &References,
    lines: &LineIndex,
    line: usize,
    text: &str,
) -> Result<(Placement, Selector), Error> {
    let matching = || {
        references.links.iter().filter_map(|link| {
            let end = link.end?;
            let Some(Ok(target)) = &link.target else {
                return None;
            };
            if target.dir || target.path.is_empty() || lines.line(end - 1) != line {
                return None;
            }
            // Matched as text: a decoded path may hold spaces or `#`.
            let location = match text.strip_prefix(target.path.as_str())? {
                "" => None,
                rest => Some(rest.strip_prefix('#').filter(|l| anchor::is_location(l))?),
            };
            let selector = Selector {
                path: target.path.clone(),
                location: location.map(str::to_string),
            };
            Some((end, selector))
        })
    };
    if let Some((end, selector)) = matching().find(|(end, _)| !references.bound.contains(end)) {
        return Ok((Placement::Bound(end), selector));
    }
    match matching().next() {
        // The read-back check refuses an end-of-line text that selects anything else.
        Some((_, selector)) => Ok((Placement::Standalone, selector)),
        None => anchor::parse_selector(text)
            .map(|selector| (Placement::Standalone, selector))
            .ok_or_else(|| Error::Usage(format!("malformed selector: {text}"))),
    }
}

/// Whether the anchor is judged against `selector`.
fn judges(anchor: &Anchor, selector: &Selector) -> bool {
    matches!(&anchor.reference, Reference::Selector(s) if s == selector)
}

pub struct DigestRequest<'a> {
    pub doc: &'a Path,
    pub line: usize,
    pub expect_selector: &'a str,
    pub basis: Basis,
}

pub enum Digested {
    Written(Mode, String),
    Unchanged,
}

/// Recomputes one digest anchor with its recorded mode and rewrites only its digest.
pub fn digest(repo: &Repo, request: DigestRequest) -> Result<Digested, Error> {
    let (doc, kind, source) = read_doc(repo, request.doc)?;
    let anchors = link::references(&doc, kind, &source).anchors;
    // Compared as text, so a bound path holding spaces or `#` still matches.
    let found: Vec<(&Anchor, &Selector)> = anchors
        .iter()
        .filter_map(|a| match &a.reference {
            Reference::Selector(s)
                if a.line == request.line && s.text() == request.expect_selector =>
            {
                Some((a, s))
            }
            _ => None,
        })
        .collect();
    let (anchor, selector) = match found.as_slice() {
        [found] => *found,
        [] => {
            return Err(Error::Refused(format!(
                "no digest anchor for {} on line {}; nothing written",
                request.expect_selector, request.line
            )));
        }
        _ => {
            return Err(Error::Refused(format!(
                "more than one digest anchor for {} on line {}; nothing written",
                request.expect_selector, request.line
            )));
        }
    };

    let snapshot = Snapshot::load(repo, request.basis)?;
    let basis = snapshot.basis().describe();
    let recorded = anchor.digest.as_ref();
    let (mode, value) = match target::digest(&snapshot, selector, recorded.map(|(m, _)| *m)) {
        Ok(digest) => digest,
        Err(Failure::Unresolved(reason)) => {
            return Err(Error::Refused(format!(
                "{}: unresolved in {basis} ({reason}); nothing written",
                request.expect_selector
            )));
        }
        Err(Failure::Read(e)) => return Err(e),
    };
    if recorded.is_some_and(|(_, old)| *old == value) {
        return Ok(Digested::Unchanged);
    }
    let token = format!("{}:{value}", mode.as_str());
    let mut updated = source.clone();
    match &anchor.digest_span {
        Some(span) => updated.replace_range(span.clone(), &token),
        // A bound whole-file anchor has no selector text; the token takes its place.
        None if anchor.selector_text.is_empty() => {
            let at = anchor.selector_span.start;
            let before = if source[..at].ends_with(char::is_whitespace) {
                ""
            } else {
                " "
            };
            let after = if source[at..].starts_with(char::is_whitespace) {
                ""
            } else {
                " "
            };
            updated.insert_str(at, &format!("{before}{token}{after}"));
        }
        None => updated.insert_str(anchor.selector_span.end, &format!(" {token}")),
    }
    write_doc(repo, &doc, &updated)?;
    Ok(Digested::Written(mode, value))
}

fn read_doc(repo: &Repo, given: &Path) -> Result<(String, DocKind, String), Error> {
    let doc = repo.relative(given)?;
    let kind = DocKind::of(&doc)
        .ok_or_else(|| Error::Usage(format!("{doc}: documents are *.md, *.mermaid, or *.mmd")))?;
    let bytes =
        std::fs::read(repo.disk_path(&doc)).map_err(|e| Error::Read(format!("{doc}: {e}")))?;
    let source = String::from_utf8(bytes).map_err(|_| Error::Read(format!("{doc}: not UTF-8")))?;
    Ok((doc, kind, source))
}

fn check_line(source: &str, lines: &LineIndex, line: usize) -> Result<(), Error> {
    let count = if source.ends_with('\n') {
        lines.count() - 1
    } else {
        lines.count()
    };
    if line == 0 || line > count {
        return Err(Error::Usage(format!(
            "--line {line} is outside the document (1..={count})"
        )));
    }
    Ok(())
}

fn write_doc(repo: &Repo, doc: &str, content: &str) -> Result<(), Error> {
    std::fs::write(repo.disk_path(doc), content).map_err(|e| Error::Read(format!("{doc}: {e}")))
}
