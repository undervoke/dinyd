//! Anchors inline in documents: grammar plus the md and .mermaid scanners.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `dinyd:` — a claim restating code facts, tied by a digest.
    Digest,
    /// `dinyd-ref:` — a delegation; only existence is checked.
    Ref,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Structural: tree-sitter tokens, whitespace between tokens ignored, comments included.
    S1,
    /// Bytes after CRLF -> LF.
    B1,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::S1 => "s1",
            Mode::B1 => "b1",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub path: String,
    pub location: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Anchor {
    pub kind: Kind,
    /// Selector text verbatim, as written in the document.
    pub selector_text: String,
    /// `None` when the anchor is malformed.
    pub selector: Option<Selector>,
    pub digest: Option<(Mode, String)>,
    /// 1-based line (LF counted) where the anchor's comment starts.
    pub line: usize,
    pub selector_span: Range<usize>,
    pub digest_span: Option<Range<usize>>,
}

impl Anchor {
    pub fn is_malformed(&self) -> bool {
        self.selector.is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    Markdown,
    Mermaid,
}

impl DocKind {
    pub fn of(path: &str) -> Option<DocKind> {
        if path.ends_with(".md") {
            Some(DocKind::Markdown)
        } else if path.ends_with(".mermaid") {
            Some(DocKind::Mermaid)
        } else {
            None
        }
    }
}

pub fn scan(kind: DocKind, src: &str) -> Vec<Anchor> {
    match kind {
        DocKind::Markdown => scan_markdown(src),
        DocKind::Mermaid => scan_mermaid(src),
    }
}

/// Anchors in HTML comments; fenced and indented code yield no HTML events, so they are skipped.
fn scan_markdown(src: &str) -> Vec<Anchor> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut html: Vec<Range<usize>> = Vec::new();
    for (event, range) in Parser::new_ext(src, options).into_offset_iter() {
        if matches!(event, Event::Html(_) | Event::InlineHtml(_)) {
            match html.last_mut() {
                Some(last) if last.end >= range.start => last.end = last.end.max(range.end),
                _ => html.push(range),
            }
        }
    }
    let lines = LineIndex::new(src);
    let mut anchors = Vec::new();
    for region in html {
        let mut at = region.start;
        while let Some(open) = src[at..region.end].find("<!--") {
            let body_start = at + open + 4;
            let Some(close) = src[body_start..region.end].find("-->") else {
                break;
            };
            let body_end = body_start + close;
            if let Some(anchor) = parse_body(src, body_start..body_end, lines.line(at + open)) {
                anchors.push(anchor);
            }
            at = body_end + 3;
        }
    }
    anchors
}

/// Anchors in own-line `%%` comments; leading frontmatter is skipped.
fn scan_mermaid(src: &str) -> Vec<Anchor> {
    let mut anchors = Vec::new();
    let mut offset = 0;
    let mut frontmatter = starts_with_frontmatter(src);
    for (index, raw) in src.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        if frontmatter {
            if index > 0 && line == "---" {
                frontmatter = false;
            }
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if line[indent..].starts_with("%%") {
            let body_start = start + indent + 2;
            if let Some(anchor) = parse_body(src, body_start..start + line.len(), index + 1) {
                anchors.push(anchor);
            }
        }
    }
    anchors
}

fn starts_with_frontmatter(src: &str) -> bool {
    let mut lines = src.split('\n').map(|l| l.trim_end_matches('\r'));
    lines.next() == Some("---") && lines.any(|l| l == "---")
}

/// Parses a comment body; `None` when it is not a dinyd anchor at all.
fn parse_body(src: &str, body: Range<usize>, line: usize) -> Option<Anchor> {
    let text = &src[body.clone()];
    let lead = text.len() - text.trim_start().len();
    let trimmed = &text[lead..];
    let (kind, keyword) = if trimmed.starts_with("dinyd:") {
        (Kind::Digest, "dinyd:")
    } else if trimmed.starts_with("dinyd-ref:") {
        (Kind::Ref, "dinyd-ref:")
    } else {
        return None;
    };
    let rest_start = body.start + lead + keyword.len();
    let rest = &src[rest_start..body.end];
    let rest_lead = rest.len() - rest.trim_start().len();
    let content_start = rest_start + rest_lead;
    let content_end = rest_start + rest.trim_end().len();
    let content_end = content_end.max(content_start);
    let content = &src[content_start..content_end];

    let mut malformed = false;
    let mut selector_end = content_end;
    let mut digest = None;
    let mut digest_span = None;
    let last_token_start = content
        .rfind(char::is_whitespace)
        .map(|i| i + content[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(0);
    let last_token = &content[last_token_start..];
    if last_token.starts_with("s1:") || last_token.starts_with("b1:") {
        let mode = if last_token.starts_with("s1:") {
            Mode::S1
        } else {
            Mode::B1
        };
        let value = &last_token[3..];
        let valid = value.len() == 16
            && value
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if kind == Kind::Ref || !valid {
            malformed = true;
        }
        digest = Some((mode, value.to_string()));
        digest_span = Some(content_start + last_token_start..content_end);
        selector_end = content_start + content[..last_token_start].trim_end().len();
    }
    let selector_text = src[content_start..selector_end].to_string();
    let selector = if malformed {
        None
    } else {
        parse_selector(&selector_text)
    };
    Some(Anchor {
        kind,
        selector_text,
        selector,
        digest,
        line,
        selector_span: content_start..selector_end,
        digest_span,
    })
}

/// `path#location`; `None` when the selector violates the path rules.
pub fn parse_selector(text: &str) -> Option<Selector> {
    let (path, location) = match text.split_once('#') {
        Some((path, location)) => (path, Some(location)),
        None => (text, None),
    };
    let path_ok = !path.is_empty()
        && !path.contains(char::is_whitespace)
        && !path.contains('\\')
        && !path.contains("-->")
        && !path.contains("%%")
        && path
            .split('/')
            .all(|segment| segment != "." && segment != "..");
    if !path_ok || location.is_some_and(|l| l.trim().is_empty()) {
        return None;
    }
    Some(Selector {
        path: path.to_string(),
        location: location.map(str::to_string),
    })
}

pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(src: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { starts }
    }

    /// 1-based line containing byte `offset`.
    pub fn line(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset)
    }

    pub fn count(&self) -> usize {
        self.starts.len()
    }

    /// Byte range of line `n` (1-based) without its line ending.
    pub fn content(&self, src: &str, n: usize) -> Range<usize> {
        let start = self.starts[n - 1];
        let end = self.starts.get(n).copied().unwrap_or(src.len());
        let line = &src[start..end];
        start..start + line.trim_end_matches(['\n', '\r']).len()
    }

    /// Byte offset just past line `n` including its line ending.
    pub fn end(&self, src: &str, n: usize) -> usize {
        self.starts.get(n).copied().unwrap_or(src.len())
    }
}
