//! Links: every repo-local destination a Markdown document links or imports must exist, and a
//! path-less anchor takes its path from the link it follows.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};

use crate::anchor::{self, Anchor, DocKind, Reference, Selector, escape};
use crate::git::{Lookup, Snapshot};

pub struct Link {
    /// The destination as written.
    pub written: String,
    /// The offset just past a link or image usage; `None` for an `@import`.
    pub end: Option<usize>,
    /// The repository path it names, or why it names none; `None` for a URL or a destination
    /// that is only a fragment or query.
    pub target: Option<Result<Target, String>>,
}

pub struct Target {
    /// Git path from the repository root; empty for the root itself.
    pub path: String,
    /// Written with a trailing slash, so only a directory satisfies it.
    pub dir: bool,
}

impl Link {
    /// The finding's selector text: marked, with control characters escaped.
    pub fn display(&self) -> String {
        format!("link {}", escape(&self.written))
    }
}

/// A document's anchors, each path-less one bound to the link it follows, and its links.
pub struct References {
    pub anchors: Vec<Anchor>,
    pub links: Vec<Link>,
    /// Ends of the link usages that anchors bound; the anchor judges such a usage.
    pub bound: BTreeSet<usize>,
}

pub fn references(doc: &str, kind: DocKind, src: &str) -> References {
    let links = match kind {
        DocKind::Markdown => scan(doc, src),
        DocKind::Mermaid => Vec::new(),
    };
    let mut anchors = anchor::scan(kind, src);
    let bound = bind(&mut anchors, &links, src);
    References {
        anchors,
        links,
        bound,
    }
}

/// Binds each path-less anchor to the link usage that ends right before its comment, with only
/// spaces or tabs between, and returns those usages' ends. A bound anchor inherits the usage's
/// path; a destination that supplies no repository file leaves it unresolved.
fn bind(anchors: &mut [Anchor], links: &[Link], src: &str) -> BTreeSet<usize> {
    let usages: BTreeMap<usize, &Link> = links
        .iter()
        .filter_map(|link| link.end.map(|end| (end, link)))
        .collect();
    let mut bound = BTreeSet::new();
    for anchor in anchors.iter_mut().filter(|a| a.binds) {
        let end = src[..anchor.start].trim_end_matches([' ', '\t']).len();
        let Some(link) = usages.get(&end) else {
            continue;
        };
        bound.insert(end);
        let Reference::Inherited(location) = &anchor.reference else {
            continue;
        };
        let location = location.clone();
        anchor.reference = match &link.target {
            Some(Ok(target)) if !target.dir && !target.path.is_empty() => {
                Reference::Selector(Selector {
                    path: target.path.clone(),
                    location,
                })
            }
            target => Reference::Unresolved {
                written: link.written.clone(),
                reason: match target {
                    None => "names no repository path".to_string(),
                    Some(Err(reason)) => reason.clone(),
                    Some(Ok(_)) => "a digest anchor needs a file".to_string(),
                },
            },
        };
    }
    bound
}

/// Link and image destinations and `@import` operands of the Markdown document at `doc`, in
/// document order. Anything in code, frontmatter, or raw HTML is not a link.
pub fn scan(doc: &str, src: &str) -> Vec<Link> {
    let mut found: Vec<(usize, Link)> = Vec::new();
    // Where an `@import` line is not one: code, frontmatter, and raw HTML, comments included.
    let mut excluded: Vec<Range<usize>> = Vec::new();
    let options = anchor::markdown_options() | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    for (event, range) in Parser::new_ext(src, options).into_offset_iter() {
        if matches!(
            event,
            Event::Start(Tag::CodeBlock(_) | Tag::MetadataBlock(_) | Tag::HtmlBlock)
                | Event::Code(_)
                | Event::InlineHtml(_)
        ) {
            excluded.push(range.clone());
        }
        if let Event::Start(
            Tag::Link {
                link_type,
                dest_url,
                ..
            }
            | Tag::Image {
                link_type,
                dest_url,
                ..
            },
        ) = event
            && matches!(
                link_type,
                LinkType::Inline | LinkType::Reference | LinkType::Collapsed | LinkType::Shortcut
            )
        {
            let target = link_target(doc, &dest_url);
            let written = dest_url.into_string();
            let end = Some(range.end);
            found.push((
                range.start,
                Link {
                    written,
                    end,
                    target,
                },
            ));
        }
    }
    for (offset, operand) in imports(src, &excluded) {
        if !operand.is_empty() && !is_url(operand) {
            let target = Some(resolve(doc, operand));
            let written = operand.to_string();
            found.push((
                offset,
                Link {
                    written,
                    end: None,
                    target,
                },
            ));
        }
    }
    found.sort_by_key(|(offset, _)| *offset);
    found.into_iter().map(|(_, link)| link).collect()
}

/// Why a link's target is missing from the snapshot, or `None` when it exists.
pub fn judge(snapshot: &Snapshot, target: &Result<Target, String>) -> Option<String> {
    let target = match target {
        Ok(target) => target,
        Err(reason) => return Some(reason.clone()),
    };
    let path = &target.path;
    let lookup = if path.is_empty() {
        Lookup::Dir
    } else {
        snapshot.lookup(path)
    };
    match lookup {
        Lookup::Dir => None,
        Lookup::File if !target.dir => None,
        Lookup::File => Some(format!("not a directory: {}", escape(path))),
        Lookup::Rejected => Some(format!("not a regular file: {}", escape(path))),
        Lookup::Missing => Some(format!("no such path: {}", escape(path))),
    }
}

/// A link destination's path, with its query and fragment ignored and percent-decoded once.
fn link_target(doc: &str, dest: &str) -> Option<Result<Target, String>> {
    if is_url(dest) {
        return None;
    }
    let path = &dest[..dest.find(['?', '#']).unwrap_or(dest.len())];
    if path.is_empty() {
        return None;
    }
    Some(percent_decode(path).and_then(|path| resolve(doc, &path)))
}

fn is_url(dest: &str) -> bool {
    dest.starts_with("//")
        || dest.split_once(':').is_some_and(|(scheme, _)| {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        })
}

fn percent_decode(path: &str) -> Result<String, String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3);
        if bytes[i] == b'%'
            && let Some(hex) = hex.filter(|h| h.iter().all(u8::is_ascii_hexdigit))
        {
            out.push(u8::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "not UTF-8 after percent-decoding".to_string())
}

/// Relative to the document's directory, or to the root after a leading `/`.
fn resolve(doc: &str, path: &str) -> Result<Target, String> {
    let mut segments: Vec<&str> = Vec::new();
    if !path.starts_with('/') {
        segments.extend(doc.split('/'));
        segments.pop();
    }
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err("leaves the repository".to_string());
                }
            }
            segment => segments.push(segment),
        }
    }
    Ok(Target {
        path: segments.join("/"),
        dir: path.ends_with('/'),
    })
}

/// Markdown Preview Enhanced's own-line `@import "path"` outside `excluded`; the operand is literal.
fn imports<'s>(src: &'s str, excluded: &[Range<usize>]) -> Vec<(usize, &'s str)> {
    let mut found = Vec::new();
    let mut start = 0;
    for line in src.split_inclusive('\n') {
        let at = start + line.len() - line.trim_start().len();
        start += line.len();
        let operand = line
            .trim_start()
            .strip_prefix("@import")
            .filter(|rest| rest.starts_with([' ', '\t']))
            .and_then(|rest| rest.trim_start().strip_prefix('"'))
            .and_then(|rest| rest.split_once('"'))
            .map(|(operand, _)| operand);
        if let Some(operand) = operand
            && !excluded.iter().any(|r| r.contains(&at))
        {
            found.push((at, operand));
        }
    }
    found
}
