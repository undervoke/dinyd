//! Anchors inline in documents: grammar plus the md and Mermaid scanners.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `dinyd:` — a claim restating code facts, tied by a digest.
    Digest,
    /// `dinyd-ref:` — a delegation; only existence is checked.
    Ref,
    /// `dinyd-id:` or `/dinyd-id:` — a unit boundary; scan yields one only when malformed.
    Declaration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Structural: tree-sitter tokens, whitespace between tokens ignored, comments included.
    S1,
    /// Bytes after CRLF -> LF.
    B1,
    /// A document unit's bytes after CRLF -> LF, with dinyd comments projected out.
    D1,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::S1 => "s1",
            Mode::B1 => "b1",
            Mode::D1 => "d1",
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
        } else if path.ends_with(".mermaid") || path.ends_with(".mmd") {
            Some(DocKind::Mermaid)
        } else {
            None
        }
    }
}

/// A comment outside code, where anchors and unit declarations live.
pub struct Comment {
    /// The whole comment: `<!--` through `-->`, or `%%` through the end of its line.
    pub extent: Range<usize>,
    body: Range<usize>,
    line: usize,
    /// False when the comment's terminator was lost.
    pub terminated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Digest,
    Ref,
    Start,
    End,
}

const KEYWORDS: [(&str, Keyword); 4] = [
    ("dinyd:", Keyword::Digest),
    ("dinyd-ref:", Keyword::Ref),
    ("dinyd-id:", Keyword::Start),
    ("/dinyd-id:", Keyword::End),
];

impl Comment {
    /// The dinyd keyword opening the body and the offset just past it.
    pub fn keyword(&self, src: &str) -> Option<(Keyword, usize)> {
        let text = &src[self.body.clone()];
        let lead = text.len() - text.trim_start().len();
        KEYWORDS
            .iter()
            .find(|(word, _)| text[lead..].starts_with(word))
            .map(|(word, keyword)| (*keyword, self.body.start + lead + word.len()))
    }

    /// A well-formed unit boundary: whether it ends the unit, and the unit's name.
    pub fn declaration<'s>(&self, src: &'s str) -> Option<(bool, &'s str)> {
        let (keyword, rest) = self.keyword(src)?;
        let name = src[rest..self.body.end].trim();
        let end = match keyword {
            Keyword::Start => false,
            Keyword::End => true,
            _ => return None,
        };
        (self.terminated && is_name(name)).then_some((end, name))
    }

    /// The anchor this comment holds; a malformed declaration counts as one so check reports it.
    fn anchor(&self, src: &str) -> Option<Anchor> {
        let (keyword, rest) = self.keyword(src)?;
        let mut anchor = match keyword {
            Keyword::Digest => parse_reference(src, Kind::Digest, rest..self.body.end, self.line),
            Keyword::Ref => parse_reference(src, Kind::Ref, rest..self.body.end, self.line),
            Keyword::Start | Keyword::End => {
                if self.declaration(src).is_some() {
                    return None;
                }
                let body = &src[self.body.clone()];
                let start = self.body.start + body.len() - body.trim_start().len();
                let text = body.trim();
                Anchor {
                    kind: Kind::Declaration,
                    selector_text: text.to_string(),
                    selector: None,
                    digest: None,
                    line: self.line,
                    selector_span: start..start + text.len(),
                    digest_span: None,
                    terminated: true,
                }
            }
        };
        if !self.terminated {
            anchor.selector = None;
            anchor.terminated = false;
        }
        Some(anchor)
    }
}

/// A unit name: ASCII letters, digits, `_`, `.`, `-`.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

pub fn scan(kind: DocKind, src: &str) -> Vec<Anchor> {
    comments(kind, src)
        .iter()
        .filter_map(|c| c.anchor(src))
        .collect()
}

pub fn comments(kind: DocKind, src: &str) -> Vec<Comment> {
    match kind {
        DocKind::Markdown => markdown_comments(src),
        DocKind::Mermaid => mermaid_comments(src),
    }
}

/// Parser options shared by every Markdown pass, so that all of them agree on what is code.
pub fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

/// HTML comments anywhere outside code. The parser supplies only the code to skip
/// (fenced and indented blocks, inline code spans): it drops some comments from its events,
/// such as one after a table row's last pipe, and an unterminated comment is not HTML to it.
fn markdown_comments(src: &str) -> Vec<Comment> {
    let mut code: Vec<Range<usize>> = Parser::new_ext(src, markdown_options())
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
    let mut comments = Vec::new();
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
                comments.push(Comment {
                    extent: open..body_end + 3,
                    body: body_start..body_end,
                    line: lines.line(open),
                    terminated: true,
                });
                at = body_end + 3;
            }
            (_, Some(next)) => {
                comments.push(unterminated(src, open, next, &lines));
                at = next;
            }
            (None, None) => {
                comments.push(unterminated(src, open, src.len(), &lines));
                break;
            }
        }
    }
    comments
}

/// A comment whose `-->` was lost still counts, as malformed. Its body is bounded to its own
/// line so that following prose never becomes selector text.
fn unterminated(src: &str, open: usize, end: usize, lines: &LineIndex) -> Comment {
    let body_start = open + 4;
    let line_end = src[body_start..end]
        .find(['\n', '\r'])
        .map_or(end, |i| body_start + i);
    Comment {
        extent: open..line_end,
        body: body_start..line_end,
        line: lines.line(body_start),
        terminated: false,
    }
}

/// Own-line `%%` comments; leading frontmatter is skipped.
fn mermaid_comments(src: &str) -> Vec<Comment> {
    let mut comments = Vec::new();
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
            comments.push(Comment {
                extent: start + indent..start + line.len(),
                body: start + indent + 2..start + line.len(),
                line: index + 1,
                terminated: true,
            });
        }
    }
    comments
}

fn starts_with_frontmatter(src: &str) -> bool {
    let mut lines = src.split('\n').map(|l| l.trim_end_matches('\r'));
    lines.next() == Some("---") && lines.any(|l| l == "---")
}

/// Parses the rest of a `dinyd:` or `dinyd-ref:` body, after its keyword.
fn parse_reference(src: &str, kind: Kind, body: Range<usize>, line: usize) -> Anchor {
    let rest_start = body.start;
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
            "d1" => Some(Mode::D1),
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
    Anchor {
        kind,
        selector_text,
        selector,
        digest,
        line,
        selector_span: content_start..selector_end,
        digest_span,
        terminated: true,
    }
}

/// A trailing `mode:value` or bare 16-hex token: digest metadata, never part of the selector,
/// so a malformed digest is not printed back as selector text.
fn is_digest_token(token: &str) -> bool {
    if ["s1:", "b1:", "d1:"].iter().any(|m| token.starts_with(m)) {
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
