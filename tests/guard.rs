//! Regressions for observed host-payload and patch-preview wiring defects.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

struct Repo(TempDir);

impl Repo {
    fn new(text: &str) -> Self {
        let repo = Self(TempDir::new().unwrap());
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success()
        );
        repo.write("doc.md", text);
        repo
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn write(&self, path: &str, text: &str) {
        std::fs::write(self.path().join(path), text).unwrap();
    }

    fn denied(&self, host: &str, input: Value) -> bool {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dinyd"))
            .args(["guard", "--host", host])
            .current_dir(self.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        serde_json::to_writer(child.stdin.take().unwrap(), &input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let verdict: Value = serde_json::from_slice(&output.stdout).unwrap();
        match host {
            "opencode" | "kilo" | "pi" => verdict["block"].as_bool().expect("plugin decision"),
            "cursor" => verdict["permission"] == "deny",
            _ => verdict["hookSpecificOutput"]["permissionDecision"] == "deny",
        }
    }

    fn patch(&self, host: &str, text: &str) -> bool {
        let input = if host == "codex" {
            json!({"tool_name":"apply_patch", "tool_input":{"command":text}})
        } else {
            json!({"tool":"apply_patch", "args":{"patchText":text}})
        };
        self.denied(host, input)
    }
}

const LOCK: &str = "<!-- dinyd-lock -->\nprotected\n<!-- /dinyd-lock -->\n";

// Appending v2's project-relative subpath inspected the wrong file or rejected valid edits.
// Retire when the OpenCode v2 adapter no longer resolves session directories.
#[test]
fn opencode_v2_uses_the_absolute_session_directory() {
    let repo = Repo::new("unlocked plugin load location\n");
    let binary_dir = Path::new(env!("CARGO_BIN_EXE_dinyd")).parent().unwrap();
    let mut paths = vec![binary_dir.to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let output = Command::new("node")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/opencode-v2.mjs"
        ))
        .arg(repo.path())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// Dropping a native patch field silently allowed edits; retire with patch-tool support.
#[test]
fn native_patch_payloads_reach_the_guard() {
    let repo = Repo::new(&format!("outside\n{LOCK}"));
    for host in ["codex", "opencode", "kilo"] {
        assert!(repo.patch(
            host,
            "*** Begin Patch\n*** Update File: doc.md\n@@\n-protected\n+changed\n*** End Patch"
        ));
        assert!(!repo.patch(
            host,
            "*** Begin Patch\n*** Update File: doc.md\n@@\n-outside\n+changed\n*** End Patch"
        ));
    }
}

// Lost array/replace-all inputs rejected valid edits; retire with those edit paths.
#[test]
fn pi_arrays_and_replace_all_use_the_actual_replacements() {
    let repo = Repo::new(&format!("repeat repeat\n{LOCK}"));
    assert!(!repo.denied("pi", json!({"tool":"edit", "args":{"path":"doc.md", "edits":[{"oldText":"repeat repeat", "newText":"outside"}]}})));
    assert!(repo.denied("pi", json!({"tool":"edit", "args":{"path":"doc.md", "edits":[{"oldText":"protected", "newText":"changed"}]}})));
    assert!(!repo.denied("claude-code", json!({"tool_name":"Edit", "tool_input":{"file_path":"doc.md", "old_string":"repeat", "new_string":"outside", "replace_all":true}})));
    assert!(!repo.denied("claude-code", json!({"tool_name":"MultiEdit", "tool_input":{"file_path":"doc.md", "edits":[{"old_string":"repeat", "new_string":"outside", "replace_all":true}]}})));
    repo.write("doc.md", &format!("protected\n{LOCK}"));
    assert!(repo.denied("claude-code", json!({"tool_name":"Edit", "tool_input":{"file_path":"doc.md", "old_string":"protected", "new_string":"changed", "replace_all":true}})));
    // Pi matches all pairs against the original; sequential preview rejected a valid swap.
    repo.write("doc.md", &format!("A B\n{LOCK}"));
    assert!(!repo.denied("pi", json!({"tool":"edit", "args":{"path":"doc.md", "edits":[{"oldText":"A", "newText":"B"}, {"oldText":"B", "newText":"A"}]}})));
}

// Losing Move-to ownership allowed a locked source to move; retire with patch moves/adds.
#[test]
fn patch_moves_and_additions_preserve_existing_files() {
    let repo = Repo::new(LOCK);
    assert!(repo.patch("codex", "*** Begin Patch\n*** Update File: doc.md\n*** Move to: other.md\n@@\n protected\n*** End Patch"));
    repo.write("free.md", "free\n");
    assert!(repo.patch("codex", "*** Begin Patch\n*** Update File: free.md\n*** Move to: doc.md\n@@\n-free\n+changed\n*** End Patch"));
    assert!(repo.patch(
        "codex",
        "*** Begin Patch\n*** Add File: doc.md\n+changed\n*** End Patch"
    ));
    let identical = format!(
        "*** Begin Patch\n*** Add File: doc.md\n+{}*** End Patch",
        LOCK.replace('\n', "\n+").trim_end_matches('+')
    );
    assert!(!repo.patch("codex", &identical));
}

// Ignoring @@ context previews the wrong repeated line; retire with patch context lookup.
#[test]
fn patch_context_and_eof_select_the_intended_occurrence() {
    let repo = Repo::new("same\n<!-- dinyd-lock -->\nsame\n<!-- /dinyd-lock -->\nsame\n");
    assert!(repo.patch("codex", "*** Begin Patch\n*** Update File: doc.md\n@@ <!-- dinyd-lock -->\n-same\n+changed\n*** End Patch"));
    assert!(!repo.patch("codex", "*** Begin Patch\n*** Update File: doc.md\n@@\n-same\n+changed\n*** End of File\n*** End Patch"));
}

// Whole-file normalization rejected a valid Codex edit; retire with its preserving preview.
#[test]
fn codex_patch_preserves_untouched_crlf_lock_bytes() {
    let repo = Repo::new(&format!("outside\n{LOCK}").replace('\n', "\r\n"));
    assert!(!repo.patch("codex", "*** Begin Patch\n*** Update File: doc.md\n@@\n-outside\n+changed\n <!-- dinyd-lock -->\n protected\n <!-- /dinyd-lock -->\n*** End Patch"));
    assert!(repo.patch(
        "codex",
        "*** Begin Patch\n*** Update File: doc.md\n@@\n-protected\n+changed\n*** End Patch"
    ));
}
