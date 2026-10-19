//! apply and update: the only writes, each touching one anchor in one document.

use std::path::Path;

use crate::Error;
use crate::anchor::{self, Anchor, DocKind, Kind, LineIndex, Mode, Selector};
use crate::git::{Basis, Repo, Snapshot};
use crate::target::{self, Failure};

pub struct ApplyRequest<'a> {
    pub doc: &'a Path,
    pub line: usize,
    pub selector: &'a str,
    pub bytes: bool,
    pub reference: bool,
    pub basis: Basis,
}

/// Writes a new anchor and returns its text with the basis its digest came from.
pub fn apply(repo: &Repo, request: ApplyRequest) -> Result<(String, String), Error> {
    let (doc, kind, source) = read_doc(repo, request.doc)?;
    let selector = anchor::parse_selector(request.selector)
        .ok_or_else(|| Error::Usage(format!("malformed selector: {}", request.selector)))?;
    if request.bytes && selector.location.is_some() && DocKind::of(&selector.path).is_some() {
        return Err(Error::Usage(
            "--bytes does not apply to a document unit; units use d1".to_string(),
        ));
    }
    let lines = LineIndex::new(&source);
    check_line(&source, &lines, request.line)?;

    let snapshot = Snapshot::load(repo, request.basis)?;
    let basis = snapshot.basis().describe();
    let resolved = if request.reference {
        target::exists(&snapshot, &selector).map(|_| None)
    } else {
        target::digest(&snapshot, &selector, request.bytes.then_some(Mode::B1)).map(Some)
    };
    let digest = match resolved {
        Ok(digest) => digest,
        Err(Failure::Unresolved(reason)) => {
            return Err(Error::Refused(format!(
                "{}: unresolved in {basis} ({reason}); nothing written",
                request.selector
            )));
        }
        Err(Failure::Read(e)) => return Err(e),
    };
    let text = match &digest {
        Some((mode, value)) => format!("dinyd: {} {}:{value}", request.selector, mode.as_str()),
        None => format!("dinyd-ref: {}", request.selector),
    };

    let target_line = match kind {
        DocKind::Markdown => request.line,
        DocKind::Mermaid => request.line + 1,
    };
    let existing = anchor::scan(kind, &source);
    if existing
        .iter()
        .any(|a| a.line == target_line && a.selector.as_ref() == Some(&selector))
    {
        return Err(Error::Refused(format!(
            "an anchor for {} already exists at line {target_line}; nothing written",
            request.selector
        )));
    }

    let content = lines.content(&source, request.line);
    let mut updated = source.clone();
    match kind {
        DocKind::Markdown => {
            let space = if content.is_empty() { "" } else { " " };
            updated.insert_str(content.end, &format!("{space}<!-- {text} -->"));
        }
        DocKind::Mermaid => {
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

    let visible = anchor::scan(kind, &updated).iter().any(|a| {
        a.line == target_line
            && a.kind
                == if digest.is_some() {
                    Kind::Digest
                } else {
                    Kind::Ref
                }
            && a.selector.as_ref() == Some(&selector)
            && a.digest.as_ref().map(|(m, v)| (*m, v.as_str()))
                == digest.as_ref().map(|(m, v)| (*m, v.as_str()))
    });
    if !visible {
        return Err(Error::Refused(format!(
            "line {} is where the anchor would not be read (code, comment, or frontmatter); nothing written",
            request.line
        )));
    }
    write_doc(repo, &doc, &updated)?;
    Ok((text, basis))
}

pub struct UpdateRequest<'a> {
    pub doc: &'a Path,
    pub line: usize,
    pub expect_selector: &'a str,
    pub basis: Basis,
}

pub enum Updated {
    Written(Mode, String),
    Unchanged,
}

/// Recomputes one digest anchor with its recorded mode and rewrites only its digest.
pub fn update(repo: &Repo, request: UpdateRequest) -> Result<Updated, Error> {
    let (doc, kind, source) = read_doc(repo, request.doc)?;
    let expected: Option<Selector> = anchor::parse_selector(request.expect_selector);
    let anchors: Vec<Anchor> = anchor::scan(kind, &source)
        .into_iter()
        .filter(|a| a.line == request.line && a.selector.is_some() && a.selector == expected)
        .collect();
    let digest_anchors: Vec<&Anchor> = anchors.iter().filter(|a| a.kind == Kind::Digest).collect();
    let anchor = match digest_anchors.as_slice() {
        [anchor] => *anchor,
        [] if !anchors.is_empty() => {
            return Err(Error::Refused(format!(
                "{} is a delegation reference; it has no digest",
                request.expect_selector
            )));
        }
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
    let (mode, value) = match target::digest(
        &snapshot,
        anchor.selector.as_ref().unwrap(),
        recorded.map(|(m, _)| *m),
    ) {
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
        return Ok(Updated::Unchanged);
    }
    let token = format!("{}:{value}", mode.as_str());
    let mut updated = source.clone();
    match &anchor.digest_span {
        Some(span) => updated.replace_range(span.clone(), &token),
        None => updated.insert_str(anchor.selector_span.end, &format!(" {token}")),
    }
    write_doc(repo, &doc, &updated)?;
    Ok(Updated::Written(mode, value))
}

fn read_doc(repo: &Repo, given: &Path) -> Result<(String, DocKind, String), Error> {
    let doc = repo.relative(given)?;
    let kind = DocKind::of(&doc)
        .ok_or_else(|| Error::Usage(format!("{doc}: documents are *.md or *.mermaid")))?;
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
