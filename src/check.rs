//! check: one verdict per anchor, a line per failure.

use std::collections::BTreeSet;
use std::fmt;

use crate::Error;
use crate::anchor::{self, Anchor, DocKind, Kind};
use crate::git::{self, Basis, Repo, Snapshot};
use crate::link;
use crate::target::{self, Failure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Malformed,
    Unresolved,
    Undeclared,
    Stale,
}

impl Verdict {
    fn next_step(self) -> &'static str {
        match self {
            Verdict::Ok => "",
            Verdict::Malformed | Verdict::Unresolved => "re-judge the claim and the reference",
            Verdict::Undeclared => "fill the digest with update",
            Verdict::Stale => "re-judge the claim, then update",
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Verdict::Ok => "ok",
            Verdict::Malformed => "malformed",
            Verdict::Unresolved => "unresolved",
            Verdict::Undeclared => "undeclared",
            Verdict::Stale => "stale",
        })
    }
}

pub struct Finding {
    pub doc: String,
    /// The anchor's selector, or `link` and the destination as written.
    pub selector: String,
    pub verdict: Verdict,
    /// Why the target did not resolve; goes to stderr.
    pub reason: Option<String>,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} → {}: {} — {}",
            self.doc,
            self.selector,
            self.verdict,
            self.verdict.next_step()
        )
    }
}

pub enum Scope {
    All,
    /// Changed against this base revision.
    Changed(String),
}

/// Failing anchors in document order, then each document's failing links.
pub fn check(repo: &Repo, basis: Basis, scope: Scope) -> Result<Vec<Finding>, Error> {
    let changed = match &scope {
        Scope::All => None,
        Scope::Changed(base) => Some(git::changed_paths(repo, &basis, base)?),
    };
    let snapshot = Snapshot::load(repo, basis)?;
    let mut findings = Vec::new();
    for doc in snapshot.documents() {
        let kind = DocKind::of(&doc).unwrap();
        let source = String::from_utf8_lossy(&snapshot.read(&doc)?).into_owned();
        for anchor in anchor::scan(kind, &source) {
            if let Some(changed) = &changed
                && !selected(&anchor, &doc, changed)
            {
                continue;
            }
            let (verdict, reason) = judge(&snapshot, &anchor)?;
            if verdict != Verdict::Ok {
                findings.push(Finding {
                    doc: doc.clone(),
                    selector: anchor.display_selector(),
                    verdict,
                    reason,
                });
            }
        }
        if kind != DocKind::Markdown {
            continue;
        }
        // Without line numbers, repeats of one destination would print identical lines.
        let mut reported = BTreeSet::new();
        for link in link::scan(&doc, &source) {
            if let Some(changed) = &changed
                && !match &link.target {
                    Ok(target) => touches(changed, &doc, &target.path),
                    Err(_) => changed.contains(&doc),
                }
            {
                continue;
            }
            if let Some(reason) = link::judge(&snapshot, &link)
                && reported.insert(link.written.clone())
            {
                findings.push(Finding {
                    doc: doc.clone(),
                    selector: link.display(),
                    verdict: Verdict::Unresolved,
                    reason: Some(reason),
                });
            }
        }
    }
    Ok(findings)
}

fn selected(anchor: &Anchor, doc: &str, changed: &BTreeSet<String>) -> bool {
    let Some(selector) = &anchor.selector else {
        return true;
    };
    touches(changed, doc, &selector.path)
}

/// Whether the document, the target path, or anything under it changed.
fn touches(changed: &BTreeSet<String>, doc: &str, path: &str) -> bool {
    // The root (an empty path) holds every changed path.
    let under = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    changed.contains(doc)
        || changed.contains(path)
        || changed
            .range(under.clone()..)
            .next()
            .is_some_and(|p| p.starts_with(&under))
}

pub fn judge(snapshot: &Snapshot, anchor: &Anchor) -> Result<(Verdict, Option<String>), Error> {
    let Some(selector) = &anchor.selector else {
        return Ok((Verdict::Malformed, None));
    };
    let result = match anchor.kind {
        // scan yields a declaration only when it is malformed, so it has no selector.
        Kind::Declaration => return Ok((Verdict::Malformed, None)),
        Kind::Ref => target::exists(snapshot, selector).map(|_| Verdict::Ok),
        Kind::Digest => {
            let recorded = anchor.digest.as_ref();
            target::digest(snapshot, selector, recorded.map(|(mode, _)| *mode)).map(|(_, value)| {
                match recorded {
                    None => Verdict::Undeclared,
                    Some((_, old)) if *old == value => Verdict::Ok,
                    Some(_) => Verdict::Stale,
                }
            })
        }
    };
    match result {
        Ok(verdict) => Ok((verdict, None)),
        Err(Failure::Unresolved(reason)) => Ok((Verdict::Unresolved, Some(reason))),
        Err(Failure::Read(e)) => Err(e),
    }
}
