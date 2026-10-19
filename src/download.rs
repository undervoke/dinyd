//! Native acquisition of pinned runtimes: fetch, verify, publish.
//!
//! Fills the version cache and serves self-update. The only external runtime
//! dependency stays the `git` executable: archives travel over the project's
//! own HTTPS client and unpack with linked libraries, never curl or tar.

use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use semver::Version;

use crate::Error;

const OWNER_REPO: &str = "undervoke/dinyd";
const USER_AGENT: &str = concat!("dinyd/", env!("CARGO_PKG_VERSION"));

/// Target triple this binary was built for, stamped by build.rs from TARGET,
/// so native and translated (Rosetta) installs never share a cache slot.
pub fn build_target() -> &'static str {
    option_env!("DINYD_BUILD_TARGET").unwrap_or("unknown")
}

/// Parse an exact version, tolerating the `v` tag prefix callers may pass.
pub fn parse_version(text: &str) -> Result<Version, Error> {
    let trimmed = text.trim();
    let plain = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(plain).map_err(|_| {
        Error::Usage(format!(
            "expected an exact version like 0.3.0, found {trimmed:?}"
        ))
    })
}

/// Root of the immutable version cache: `<cache>/dinyd`.
pub fn cache_root() -> Result<PathBuf, Error> {
    let base = dirs::cache_dir()
        .ok_or_else(|| Error::Read("cannot locate the user cache directory".to_string()))?;
    Ok(base.join("dinyd"))
}

/// Path of the cached executable for `version` on `target`.
pub fn cached_executable(version: &Version, target: &str) -> Result<PathBuf, Error> {
    let mut path = cache_root()?;
    path.push(version.to_string());
    path.push(target);
    path.push(exe_name());
    Ok(path)
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "dinyd.exe" } else { "dinyd" }
}

fn archive_name(target: &str) -> String {
    if cfg!(windows) {
        format!("dinyd-{target}.zip")
    } else {
        format!("dinyd-{target}.tar.xz")
    }
}

/// Newest stable release; `releases/latest` already excludes drafts and prereleases.
pub fn latest_stable() -> Result<Version, Error> {
    let url = format!("https://api.github.com/repos/{OWNER_REPO}/releases/latest");
    let body = get_text(&url, "application/vnd.github+json")?;
    let value = serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| Error::Read(format!("release lookup: {e}")))?;
    let tag = value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Read("release lookup: no tag_name".to_string()))?;
    parse_version(tag)
}

/// Cached executable for `version` on `target`, downloading on a miss.
/// A concurrent hook may publish first; only a complete file is ever visible.
pub fn fetch_release(version: &Version, target: &str) -> Result<PathBuf, Error> {
    if target == "unknown" {
        return Err(Error::Usage(
            "DINYD_BUILD_TARGET is unset; rebuild with build.rs".to_string(),
        ));
    }
    let dest = cached_executable(version, target)?;
    if dest.is_file() {
        return Ok(dest);
    }
    let archive = archive_name(target);
    let base_url =
        format!("https://github.com/{OWNER_REPO}/releases/download/v{version}/{archive}");
    let sidecar = get_bytes(&format!("{base_url}.sha256"), "application/octet-stream")?;
    let expected = checksum_of(&sidecar)?;
    let bytes = get_bytes(&base_url, "application/octet-stream")?;
    verify(&bytes, &expected, &base_url)?;
    install_bytes(&dest, &extract_binary(&bytes)?)?;
    Ok(dest)
}

/// Atomically publish `bytes` as an executable at `dest`; a present file wins,
/// so concurrent hooks never observe a partial runtime.
pub fn install_bytes(dest: &Path, bytes: &[u8]) -> Result<(), Error> {
    if dest.is_file() {
        return Ok(());
    }
    let dir = dest
        .parent()
        .ok_or_else(|| Error::Read(format!("{} has no parent directory", dest.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| Error::Read(format!("{}: {e}", dir.display())))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| Error::Read(format!("{}: {e}", dir.display())))?;
    tmp.write_all(bytes)
        .and_then(|()| tmp.flush())
        .map_err(|e| Error::Read(format!("{}: {e}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))
            .map_err(|e| Error::Read(format!("{}: {e}", dest.display())))?;
    }
    match tmp.persist_noclobber(dest) {
        Ok(_) => Ok(()),
        // A concurrent hook published between the check and the rename.
        Err(_) if dest.is_file() => Ok(()),
        Err(e) => Err(Error::Read(format!("{}: {e}", dest.display()))),
    }
}

fn get_bytes(url: &str, accept: &str) -> Result<Vec<u8>, Error> {
    let mut response = ureq::agent()
        .get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", accept)
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => Error::Read(format!("not found: {url}")),
            other => Error::Read(format!("GET {url}: {other}")),
        })?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Read(format!("GET {url}: {e}")))?;
    Ok(bytes)
}

fn get_text(url: &str, accept: &str) -> Result<String, Error> {
    let bytes = get_bytes(url, accept)?;
    String::from_utf8(bytes).map_err(|e| Error::Read(format!("GET {url}: {e}")))
}

/// First token of the `.sha256` sidecar, the digest the release publishes.
fn checksum_of(sidecar: &[u8]) -> Result<String, Error> {
    let text = std::str::from_utf8(sidecar)
        .map_err(|_| Error::Read("checksum sidecar is not UTF-8".to_string()))?;
    let hex = text
        .split_whitespace()
        .next()
        .ok_or_else(|| Error::Read("checksum sidecar is empty".to_string()))?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Read(
            "checksum sidecar carries no SHA-256 digest".to_string(),
        ));
    }
    Ok(hex.to_ascii_lowercase())
}

/// The complete archive hashes before anything is unpacked or published;
/// the short document digest is unrelated to executable integrity.
fn verify(bytes: &[u8], expected: &str, url: &str) -> Result<(), Error> {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    let actual: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err(Error::Read(format!(
            "{url}: checksum mismatch (want {expected}, got {actual})"
        )));
    }
    Ok(())
}

/// The one executable inside the release archive; nothing else is unpacked.
fn extract_binary(archive: &[u8]) -> Result<Vec<u8>, Error> {
    #[cfg(unix)]
    {
        let decoder = xz2::read::XzDecoder::new(archive);
        let mut tar = tar::Archive::new(decoder);
        let entries = tar
            .entries()
            .map_err(|e| Error::Read(format!("archive: {e}")))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| Error::Read(format!("archive: {e}")))?;
            let wanted = entry
                .path()
                .map(|p| {
                    p.file_name().is_some_and(|n| n.to_str() == Some("dinyd"))
                        && entry.header().entry_type().is_file()
                })
                .unwrap_or(false);
            if wanted {
                let mut out = Vec::new();
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| Error::Read(format!("archive: {e}")))?;
                return Ok(out);
            }
        }
        Err(Error::Read("archive holds no dinyd executable".to_string()))
    }
    #[cfg(windows)]
    {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
            .map_err(|e| Error::Read(format!("archive: {e}")))?;
        for i in 0..zip.len() {
            let mut file = zip
                .by_index(i)
                .map_err(|e| Error::Read(format!("archive: {e}")))?;
            let wanted = !file.is_dir()
                && file.enclosed_name().is_some_and(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_str() == Some("dinyd.exe"))
                });
            if wanted {
                let mut out = Vec::new();
                file.read_to_end(&mut out)
                    .map_err(|e| Error::Read(format!("archive: {e}")))?;
                return Ok(out);
            }
        }
        Err(Error::Read(
            "archive holds no dinyd.exe executable".to_string(),
        ))
    }
}
