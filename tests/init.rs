use std::path::Path;
use std::process::{Command, Output};

use serde_json::json;
use tempfile::TempDir;

struct Project {
    repo: TempDir,
    home: TempDir,
}

impl Project {
    fn new() -> Self {
        let project = Self {
            repo: TempDir::new().unwrap(),
            home: TempDir::new().unwrap(),
        };
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(project.path())
                .status()
                .unwrap()
                .success()
        );
        project
    }

    fn installed_runtime(&self) {
        // Supply an installed runtime fixture so settings regressions exercise the CLI offline.
        let cache = if cfg!(target_os = "macos") {
            self.home.path().join("Library/Caches")
        } else {
            self.home.path().to_path_buf()
        };
        let executable = cache
            .join("dinyd")
            .join(env!("CARGO_PKG_VERSION"))
            .join(env!("DINYD_BUILD_TARGET"))
            .join(if cfg!(windows) { "dinyd.exe" } else { "dinyd" });
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_dinyd"), executable).unwrap();
    }

    fn path(&self) -> &Path {
        self.repo.path()
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn init(&self, hosts: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dinyd"))
            .args(["init", "--hosts", hosts])
            .current_dir(self.path())
            .env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path())
            .env("LOCALAPPDATA", self.home.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }
}

// JSON merge fallback erased settings; retire when init no longer merges shared hook files.
#[test]
#[cfg_attr(
    windows,
    ignore = "Windows Known Folder cache cannot be isolated by environment variables"
)]
fn shared_hook_settings_survive_init() {
    let project = Project::new();
    project.installed_runtime();
    let path = ".claude/settings.json";
    let malformed =
        r#"{"hooks":{"PreToolUse":[{"matcher":"Edit|Write|MultiEdit","hooks":"custom"}]}}"#;
    project.write(path, malformed);
    for hosts in ["claude-code", "none"] {
        assert!(!project.init(hosts).status.success());
        assert_eq!(
            std::fs::read_to_string(project.path().join(path)).unwrap(),
            malformed
        );
        assert!(!project.path().join(".dinyd-version").exists());
    }
    let custom = json!({"matcher":"Edit|Write|MultiEdit", "hooks":[{"type":"command", "command":"my-validator"}]});
    let empty = json!({"matcher":"Read", "hooks":[]});
    project.write(
        path,
        &json!({"permissions":{"allow":["Read"]}, "hooks":{"PreToolUse":[custom, empty]}})
            .to_string(),
    );
    assert!(project.init("claude-code").status.success());
    assert!(project.init("none").status.success());
    let kept: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.path().join(path)).unwrap()).unwrap();
    assert_eq!(
        kept,
        json!({"permissions":{"allow":["Read"]}, "hooks":{"PreToolUse":[custom, empty]}})
    );
}

// Late plugin discovery overwrote a foreign file after pinning; retire with owned-plugin setup.
#[test]
fn foreign_plugin_conflict_precedes_project_writes() {
    let project = Project::new();
    let path = ".pi/extensions/dinyd-guard.ts";
    project.write(path, "export default function mine() {}\n");
    assert!(!project.init("claude-code,pi").status.success());
    assert!(!project.path().join(".dinyd-version").exists());
    assert!(!project.path().join(".claude/settings.json").exists());
    assert_eq!(
        std::fs::read_to_string(project.path().join(path)).unwrap(),
        "export default function mine() {}\n"
    );
}
