//! Select the project runtime before parsing its command-specific arguments.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use semver::Version;

use crate::Error;
use crate::download;

const DELEGATED_ENV: &str = "DINYD_DELEGATED";
const PIN_FILE: &str = ".dinyd-version";

fn running_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is semver")
}

/// Git root of `cwd`.
pub fn project_root(cwd: &Path) -> Result<PathBuf, Error> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .current_dir(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| Error::Read(format!("cannot run git: {e}")))?;
    if !output.status.success() {
        return Err(Error::Read(format!(
            "{}: not inside a Git working tree",
            cwd.display()
        )));
    }
    let root = String::from_utf8(output.stdout)
        .map_err(|e| Error::Read(format!("git root is not UTF-8: {e}")))?;
    Ok(PathBuf::from(root.trim_end_matches(['\n', '\r'])))
}

/// Pinned version from the project's `.dinyd-version`; `None` when unpinned.
pub fn read_pin(root: &Path) -> Result<Option<Version>, Error> {
    let path = root.join(PIN_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Read(format!("{}: {e}", path.display()))),
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Error::Usage(format!(
            "{}: empty; want one exact version like 0.3.0",
            path.display()
        )));
    }
    download::parse_version(trimmed)
        .map(Some)
        .map_err(|e| Error::Usage(format!("{}: {e}", path.display())))
}

/// Before command parsing. `args` includes argv[0]; `hook` carries the host
/// name and the buffered stdin only for `guard`.
pub fn dispatch(args: &[OsString], hook: Option<(&str, &[u8])>) -> Result<Option<i32>, Error> {
    if args
        .iter()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        return Ok(None);
    }
    if std::env::var_os(DELEGATED_ENV).is_some() {
        return Ok(None);
    }
    match args.get(1).map(|s| s.to_string_lossy()).as_deref() {
        // `update` always runs in the invoked installation, never the pin.
        Some("update" | "help") => Ok(None),
        Some("guard") => dispatch_guard(args, hook),
        Some("init") => dispatch_init(args),
        Some(command) if !command.starts_with('-') => {
            // Development exercises the local build, not the pinned release.
            if cfg!(debug_assertions) {
                return Ok(None);
            }
            dispatch_ordinary(args)
        }
        // Top-level flags stay local for clap to answer.
        _ => Ok(None),
    }
}

/// Ordinary commands run in the pinned runtime, acquiring it on a miss.
fn dispatch_ordinary(args: &[OsString]) -> Result<Option<i32>, Error> {
    let cwd = std::env::current_dir().map_err(|e| Error::Read(e.to_string()))?;
    let root = project_root(&cwd)?;
    match read_pin(&root)? {
        None => Ok(None),
        Some(pin) if pin == running_version() => Ok(None),
        Some(pin) => {
            let exe = download::fetch_release(&pin, download::build_target())?;
            spawn_selected(&exe, args.get(1..).unwrap_or_default(), None)
        }
    }
}

/// Setup is performed by the selected runtime so its pin and adapters agree.
fn dispatch_init(args: &[OsString]) -> Result<Option<i32>, Error> {
    let Some(spec) = init_version_spec(args) else {
        return dispatch_ordinary(args);
    };
    let exact = if spec == "latest" {
        download::latest_stable()?
    } else {
        download::parse_version(&spec)?
    };
    if exact == running_version() {
        if spec == exact.to_string() {
            return Ok(None);
        }
        let exe = std::env::current_exe().map_err(|e| Error::Read(e.to_string()))?;
        return spawn_selected(&exe, &rewrite_version(args, &exact), None);
    }
    let exe = download::fetch_release(&exact, download::build_target())?;
    spawn_selected(&exe, &rewrite_version(args, &exact), None)
}

/// Value of the first `--version <spec>` or `--version=<spec>` after `init`.
fn init_version_spec(args: &[OsString]) -> Option<String> {
    let mut rest = args.iter().skip(2);
    while let Some(arg) = rest.next() {
        let text = arg.to_string_lossy();
        if text == "--version" {
            return rest.next().map(|v| v.to_string_lossy().into_owned());
        }
        if let Some(value) = text.strip_prefix("--version=") {
            return Some(value.to_owned());
        }
    }
    None
}

/// Caller's arguments after argv[0] with the `--version` value replaced by
/// the resolved exact version, so the selected runtime parses one spelling.
fn rewrite_version(args: &[OsString], exact: &Version) -> Vec<OsString> {
    let mut out = Vec::with_capacity(args.len());
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        let text = arg.to_string_lossy();
        if text == "--version" {
            out.push(arg.clone());
            if rest.next().is_some() {
                out.push(OsString::from(exact.to_string()));
            }
            continue;
        }
        if text.starts_with("--version=") {
            out.push(OsString::from(format!("--version={exact}")));
            continue;
        }
        out.push(arg.clone());
    }
    out
}

/// One selected runtime judges the whole tool call; a guard cache miss denies
/// natively without downloading inside a pre-execution hook.
fn dispatch_guard(args: &[OsString], hook: Option<(&str, &[u8])>) -> Result<Option<i32>, Error> {
    if cfg!(debug_assertions) {
        return Ok(None);
    }
    let Some((host, raw)) = hook else {
        // No buffered payload; the in-process guard reports it.
        return Ok(None);
    };
    // A host may run its hook in the main checkout while the agent edits
    // another worktree, so the session context picks the project, not the
    // process cwd. Anything unresolvable stays local for the in-process
    // guard to report through its own path.
    let fallback = std::env::current_dir().map_err(|e| Error::Read(e.to_string()))?;
    let base = match serde_json::from_slice::<serde_json::Value>(raw) {
        Ok(payload) => crate::guard::context(&payload, &fallback),
        Err(_) => fallback,
    };
    let root = match project_root(&base) {
        Ok(root) => root,
        Err(_) => return Ok(None),
    };
    let pin = read_pin(&root)?;
    match pin {
        None => Ok(None),
        Some(pin) if pin == running_version() => Ok(None),
        Some(pin) => {
            let exe = download::cached_executable(&pin, download::build_target())?;
            if exe.is_file() {
                spawn_selected(&exe, args.get(1..).unwrap_or_default(), Some(raw))
            } else {
                let reason = format!("dinyd {pin} is not installed; run dinyd init");
                let (response, code) = crate::guard::response(host, Some(&reason));
                println!("{response}");
                Ok(Some(i32::from(code)))
            }
        }
    }
}

/// Run the selected runtime with the caller's arguments. `stdin_bytes`
/// replaces the child's stdin (guard forwards the buffered hook payload);
/// otherwise stdin is inherited. The child's status passes through.
fn spawn_selected(
    exe: &Path,
    argv: &[OsString],
    stdin_bytes: Option<&[u8]>,
) -> Result<Option<i32>, Error> {
    let mut child = Command::new(exe)
        .args(argv)
        .env(DELEGATED_ENV, "1")
        .stdin(if stdin_bytes.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| Error::Read(format!("cannot run {}: {e}", exe.display())))?;
    if let (Some(bytes), Some(mut stdin)) = (stdin_bytes, child.stdin.take()) {
        stdin
            .write_all(bytes)
            .map_err(|e| Error::Read(format!("cannot forward hook payload: {e}")))?;
    }
    let status = child
        .wait()
        .map_err(|e| Error::Read(format!("cannot run {}: {e}", exe.display())))?;
    Ok(Some(status.code().unwrap_or(1)))
}

/// Replace the invoked installation with the newest stable release.
/// Refuses cache slots: a cached runtime keeps its version identity.
pub fn self_update() -> Result<(), Error> {
    let current = std::env::current_exe()
        .map_err(|e| Error::Read(format!("cannot locate current executable: {e}")))?;
    if under_cache(&current)? {
        return Err(Error::Usage(format!(
            "cannot self-update {}; rerun from an installation outside the version cache",
            current.display()
        )));
    }
    let latest = download::latest_stable()?;
    let running = running_version();
    if latest <= running {
        println!("dinyd {running} is already the newest stable release");
        return Ok(());
    }
    // The cache already holds the verified executable; move it over the
    // invoked one with a primitive that supports a running Windows image.
    let executable = download::fetch_release(&latest, download::build_target())?;
    self_replace::self_replace(&executable)
        .map_err(|e| Error::Read(format!("cannot replace {}: {e}", current.display())))?;
    println!("dinyd updated from {running} to {latest}");
    Ok(())
}

/// True when `exe` already lives in the immutable version cache.
fn under_cache(exe: &Path) -> Result<bool, Error> {
    let Ok(root) = download::cache_root() else {
        return Ok(false);
    };
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let root = root.canonicalize().unwrap_or(root);
    Ok(exe.starts_with(root))
}
