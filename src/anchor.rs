//! Anchors inline in documents: grammar plus the md and .mermaid scanners.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

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
    /// False when the comment's terminator was lost.
    pub terminated: bool,
}

impl Anchor {
    /// Selector text for output, never carrying digest metadata or prose. A malformed anchor in
    /// a closed one-line comment shows its non-digest tokens; one whose comment lost its
    /// terminator or spans lines shows only the first, since what follows may be prose.
    pub fn display_selector(&self) -> String {
        if self.selector.is_some() {
            return self.selector_text.clone();
        }
        let mut tokens = self
            .selector_text
            .split_whitespace()
            .filter(|t| !is_digest_token(t));
        if self.terminated && !self.selector_text.contains(['\n', '\r']) {
            tokens.collect::<Vec<_>>().join(" ")
        } else {
            tokens.next().unwrap_or("").to_string()
        }
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

/// Anchors in HTML comments anywhere outside code. The parser supplies only the code to skip
/// (fenced and indented blocks, inline code spans): it drops some comments from its events,
/// such as one after a table row's last pipe, and an unterminated comment is not HTML to it.
fn scan_markdown(src: &str) -> Vec<Anchor> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut code: Vec<Range<usize>> = Parser::new_ext(src, options)
        .into_offset_iter()
        .filter(|(event, _)| matches!(event, Event::Start(Tag::CodeBlock(_)) | Event::Code(_)))
        .map(|(_, range)| range)
        .collect();
    code.sort_by_key(|r| r.start);
    let in_code = |offset: usize| {
        let i = code.partition_point(|r| r.end <= offset);
        code.get(i).filter(|r| r.start <= offset).map(|r| r.end)
    };

    let lines = LineIndex::new(src);
    let mut anchors = Vec::new();
    let mut at = 0;
    while let Some(found) = src[at..].find("<!--") {
        let open = at + found;
        if let Some(code_end) = in_code(open) {
            at = code_end;
            continue;
        }
        let body_start = open + 4;
        // An odd run of backslashes escapes `<`: a literal example, not a comment.
        let escapes = src[..open]
            .bytes()
            .rev()
            .take_while(|&b| b == b'\\')
            .count();
        if escapes % 2 == 1 {
            at = body_start;
            continue;
        }
        let close = src[body_start..].find("-->").map(|i| body_start + i);
        // `<!--` cannot occur inside a comment: an earlier one means this one lost its terminator.
        let nested = src[body_start..close.unwrap_or(src.len())]
            .find("<!--")
            .map(|i| body_start + i);
        match (close, nested) {
            (Some(body_end), None) => {
                anchors.extend(parse_body(src, body_start..body_end, lines.line(open)));
                at = body_end + 3;
            }
            (_, Some(next)) => {
                anchors.extend(unterminated(src, body_start, next, &lines));
                at = next;
            }
            (None, None) => {
                anchors.extend(unterminated(src, body_start, src.len(), &lines));
                break;
            }
        }
    }
    anchors
}

/// An anchor whose `-->` was lost still counts, as malformed. Its body is bounded to its own
/// line so that following prose never becomes selector text.
fn unterminated(src: &str, body_start: usize, end: usize, lines: &LineIndex) -> Option<Anchor> {
    let line_end = src[body_start..end]
        .find(['\n', '\r'])
        .map_or(end, |i| body_start + i);
    let mut anchor = parse_body(src, body_start..line_end, lines.line(body_start))?;
    anchor.selector = None;
    anchor.terminated = false;
    Some(anchor)
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
    if is_digest_token(last_token) {
        let (mode, value) = last_token.split_once(':').unwrap_or(("", last_token));
        let mode = match mode {
            "s1" => Some(Mode::S1),
            "b1" => Some(Mode::B1),
            _ => None,
        };
        let valid = value.len() == 16
            && value
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        match mode {
            Some(mode) if valid && kind == Kind::Digest => digest = Some((mode, value.to_string())),
            _ => malformed = true,
        }
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
        terminated: true,
    })
}

/// A trailing `mode:value` or bare 16-hex token: digest metadata, never part of the selector,
/// so a malformed digest is not printed back as selector text.
fn is_digest_token(token: &str) -> bool {
    if token.starts_with("s1:") || token.starts_with("b1:") {
        return true;
    }
    let hex = |v: &str| !v.is_empty() && v.bytes().all(|b| b.is_ascii_hexdigit());
    match token.split_once(':') {
        Some((mode, value)) => {
            !mode.is_empty() && mode.bytes().all(|b| b.is_ascii_alphanumeric()) && hex(value)
        }
        None => token.len() == 16 && hex(token),
    }
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
    let location_ok = location.is_none_or(|l| !l.trim().is_empty() && !l.contains(['\n', '\r']));
    if !path_ok || !location_ok {
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
