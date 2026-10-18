//! Judgment basis: the index, a commit, or the working tree, read through the Git CLI.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Basis {
    Index,
    Worktree,
    Rev(String),
}

impl Basis {
    pub fn describe(&self) -> String {
        match self {
            Basis::Index => "--index".to_string(),
            Basis::Worktree => "--worktree".to_string(),
            Basis::Rev(rev) => format!("--rev {rev}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    File(String),
    Symlink,
    Gitlink,
    IntentToAdd,
    Unmerged,
}

/// What a selector path names in the basis.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    File,
    Dir,
    /// Present but not a target dinyd accepts (symlink, gitlink, intent-to-add, unmerged).
    Rejected,
    Missing,
}

pub struct Repo {
    root: PathBuf,
}

impl Repo {
    pub fn discover() -> Result<Repo, Error> {
        let out = run_git(None, &["rev-parse", "--show-toplevel"])?;
        let root = String::from_utf8_lossy(&out)
            .trim_end_matches(['\n', '\r'])
            .to_string();
        Ok(Repo {
            root: PathBuf::from(root),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn git(&self, args: &[&str]) -> Result<Vec<u8>, Error> {
        run_git(Some(&self.root), args)
    }

    /// Repository-relative Git path of a path given on the command line.
    pub fn relative(&self, given: &Path) -> Result<String, Error> {
        let absolute = std::env::current_dir()
            .map_err(|e| Error::Read(e.to_string()))?
            .join(given);
        let absolute = absolute
            .canonicalize()
            .map_err(|e| Error::Read(format!("{}: {e}", given.display())))?;
        let root = self
            .root
            .canonicalize()
            .map_err(|e| Error::Read(e.to_string()))?;
        let relative = absolute
            .strip_prefix(&root)
            .map_err(|_| Error::Usage(format!("{} is outside the repository", given.display())))?;
        let parts: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        Ok(parts.join("/"))
    }

    pub fn disk_path(&self, git_path: &str) -> PathBuf {
        let mut path = self.root.clone();
        path.extend(git_path.split('/'));
        path
    }

    pub fn verify_rev(&self, rev: &str) -> Result<(), Error> {
        self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ])
        .map(|_| ())
        .map_err(|_| Error::Usage(format!("not a commit: {rev}")))
    }
}

fn run_git(dir: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, Error> {
    let mut command = git_command(dir);
    command.args(args);
    let output = command
        .output()
        .map_err(|e| Error::Read(format!("cannot run git: {e}")))?;
    if !output.status.success() {
        return Err(Error::Read(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn git_command(dir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command.arg("--no-optional-locks");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    // Git resolves a relative GIT_INDEX_FILE against its own working directory, which is the root here.
    if let Some(index) = std::env::var_os("GIT_INDEX_FILE")
        && let Ok(cwd) = std::env::current_dir()
    {
        command.env("GIT_INDEX_FILE", cwd.join(index));
    }
    command
}

/// The documents and targets of one basis.
pub struct Snapshot<'r> {
    repo: &'r Repo,
    basis: Basis,
    entries: BTreeMap<String, Entry>,
    dirs: BTreeSet<String>,
    cat: RefCell<Option<CatFile>>,
}

const EMPTY_BLOBS: [&str; 2] = [
    "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
    "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813",
];

impl<'r> Snapshot<'r> {
    pub fn load(repo: &'r Repo, basis: Basis) -> Result<Snapshot<'r>, Error> {
        let entries = match &basis {
            Basis::Index | Basis::Worktree => index_entries(repo)?,
            Basis::Rev(rev) => {
                repo.verify_rev(rev)?;
                tree_entries(repo, rev)?
            }
        };
        let mut dirs = BTreeSet::new();
        for path in entries.keys() {
            let mut rest = path.as_str();
            while let Some((parent, _)) = rest.rsplit_once('/') {
                dirs.insert(parent.to_string());
                rest = parent;
            }
        }
        Ok(Snapshot {
            repo,
            basis,
            entries,
            dirs,
            cat: RefCell::new(None),
        })
    }

    pub fn basis(&self) -> &Basis {
        &self.basis
    }

    /// Tracked `*.md` and `*.mermaid` documents readable from this basis.
    pub fn documents(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(path, _)| crate::anchor::DocKind::of(path).is_some())
            .filter(|(path, entry)| match self.basis {
                // Unmerged documents are read from disk too: their anchors are there.
                Basis::Worktree => {
                    !matches!(entry, Entry::Gitlink)
                        && std::fs::symlink_metadata(self.repo.disk_path(path))
                            .is_ok_and(|m| m.is_file())
                }
                _ => matches!(entry, Entry::File(_)),
            })
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn lookup(&self, path: &str) -> Lookup {
        match self.basis {
            Basis::Worktree => {
                if matches!(
                    self.entries.get(path),
                    Some(Entry::Unmerged | Entry::Gitlink)
                ) {
                    return Lookup::Rejected;
                }
                match std::fs::symlink_metadata(self.repo.disk_path(path)) {
                    Ok(meta) if meta.file_type().is_symlink() => Lookup::Rejected,
                    Ok(meta) if meta.is_dir() => Lookup::Dir,
                    Ok(meta) if meta.is_file() => Lookup::File,
                    Ok(_) => Lookup::Rejected,
                    Err(_) => Lookup::Missing,
                }
            }
            _ => match self.entries.get(path) {
                Some(Entry::File(_)) => Lookup::File,
                Some(_) => Lookup::Rejected,
                None if self.dirs.contains(path) => Lookup::Dir,
                None => Lookup::Missing,
            },
        }
    }

    /// Content of a path for which `lookup` returned `File`, or a document.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, Error> {
        match (&self.basis, self.entries.get(path)) {
            (Basis::Worktree, _) => std::fs::read(self.repo.disk_path(path))
                .map_err(|e| Error::Read(format!("{path}: {e}"))),
            (_, Some(Entry::File(oid))) => {
                let mut cat = self.cat.borrow_mut();
                if cat.is_none() {
                    *cat = Some(CatFile::spawn(self.repo)?);
                }
                cat.as_mut().unwrap().read(oid)
            }
            _ => Err(Error::Read(format!(
                "{path}: not a regular file in {}",
                self.basis.describe()
            ))),
        }
    }
}

fn index_entries(repo: &Repo) -> Result<BTreeMap<String, Entry>, Error> {
    let out = repo.git(&["ls-files", "--stage", "-z"])?;
    let mut entries = BTreeMap::new();
    let mut maybe_intent = false;
    for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let record = String::from_utf8_lossy(record);
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        let mut fields = meta.split(' ');
        let (mode, oid, stage) = (fields.next(), fields.next(), fields.next());
        let entry = if stage != Some("0") {
            Entry::Unmerged
        } else {
            maybe_intent |= oid.is_some_and(|o| EMPTY_BLOBS.contains(&o));
            entry_for(mode.unwrap_or(""), oid.unwrap_or(""))
        };
        if entries.get(path) != Some(&Entry::Unmerged) {
            entries.insert(path.to_string(), entry);
        }
    }
    // Intent-to-add entries carry the empty blob, so status is consulted only when one appears.
    if maybe_intent {
        for path in intent_to_add(repo)? {
            if let Some(entry @ Entry::File(_)) = entries.get_mut(&path) {
                *entry = Entry::IntentToAdd;
            }
        }
    }
    Ok(entries)
}

fn intent_to_add(repo: &Repo) -> Result<Vec<String>, Error> {
    let out = repo.git(&[
        "status",
        "--porcelain=v2",
        "-z",
        "--no-renames",
        "--untracked-files=no",
        "--ignore-submodules=all",
    ])?;
    let mut paths = Vec::new();
    for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let record = String::from_utf8_lossy(record);
        let fields: Vec<&str> = record.splitn(9, ' ').collect();
        // `1 XY sub mH mI mW hH hI path`: an intent-to-add entry has no index mode.
        if fields.len() == 9 && fields[0] == "1" && fields[4] == "000000" {
            paths.push(fields[8].to_string());
        }
    }
    Ok(paths)
}

fn tree_entries(repo: &Repo, rev: &str) -> Result<BTreeMap<String, Entry>, Error> {
    let out = repo.git(&["ls-tree", "-r", "-z", "--full-tree", rev])?;
    let mut entries = BTreeMap::new();
    for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let record = String::from_utf8_lossy(record);
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        let mut fields = meta.split(' ');
        let (mode, _kind, oid) = (fields.next(), fields.next(), fields.next());
        entries.insert(
            path.to_string(),
            entry_for(mode.unwrap_or(""), oid.unwrap_or("")),
        );
    }
    Ok(entries)
}

fn entry_for(mode: &str, oid: &str) -> Entry {
    match mode {
        "120000" => Entry::Symlink,
        "160000" => Entry::Gitlink,
        _ => Entry::File(oid.to_string()),
    }
}

/// Paths that differ between `base` and the basis, deletions and both sides of renames included.
pub fn changed_paths(repo: &Repo, basis: &Basis, base: &str) -> Result<BTreeSet<String>, Error> {
    repo.verify_rev(base)?;
    let out = match basis {
        Basis::Index => repo.git(&[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--cached",
            base,
        ])?,
        Basis::Worktree => repo.git(&["diff", "--name-only", "-z", "--no-renames", base])?,
        Basis::Rev(rev) => repo.git(&["diff", "--name-only", "-z", "--no-renames", base, rev])?,
    };
    Ok(out
        .split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .map(|r| String::from_utf8_lossy(r).into_owned())
        .collect())
}

struct CatFile {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl CatFile {
    fn spawn(repo: &Repo) -> Result<CatFile, Error> {
        let mut child = git_command(Some(repo.root()))
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Read(format!("cannot run git cat-file: {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(CatFile {
            child,
            stdin,
            stdout,
        })
    }

    fn read(&mut self, oid: &str) -> Result<Vec<u8>, Error> {
        let fail = |e: std::io::Error| Error::Read(format!("git cat-file: {e}"));
        writeln!(self.stdin, "{oid}").map_err(fail)?;
        self.stdin.flush().map_err(fail)?;
        let mut header = String::new();
        self.stdout.read_line(&mut header).map_err(fail)?;
        let size: usize = header
            .trim_end()
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Read(format!("git cat-file: {}", header.trim_end())))?;
        let mut content = vec![0; size + 1];
        self.stdout.read_exact(&mut content).map_err(fail)?;
        content.pop();
        Ok(content)
    }
}

impl Drop for CatFile {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
