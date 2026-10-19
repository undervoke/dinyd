//! Project setup: pin the runtime version and converge agent-host bindings.
//!
//! Version selection and delegation live in `runtime`; lock policy and the
//! `guard` command live in the guard module. This module only resolves which
//! hosts to configure, preflights every managed setting, then writes the pin
//! file and the DinyD-owned entries. It never touches entries it does not own.

use std::collections::BTreeSet;
use std::io::{BufRead, IsTerminal};
use std::path::Path;

use crate::Error;

/// Host slugs accepted by `--hosts` and `guard --host`, in display order.
use crate::guard::HOSTS as HOST_SLUGS;

const HOST_LABELS: [&str; 6] = [
    "Claude Code",
    "Codex Desktop",
    "Cursor Agent",
    "OpenCode",
    "Pi",
    "Kilo Code",
];

/// Marks a plugin file as DinyD-owned. Every adapter template carries it.
const MANAGED_MARKER: &str = "dinyd-guard: managed by";
/// A hook registration is DinyD-owned when its command starts with this.
const GUARD_PREFIX: &str = "dinyd guard";

const PIN_FILE: &str = ".dinyd-version";

/// Prepare the runtime pin and converge the project's host settings.
///
/// `version` is the exact `--version` the dispatcher already delegated to, or
/// `None` to preserve the existing pin. `hosts` is the raw `--hosts` value,
/// or `None` to keep the existing host set (asking interactively on a TTY).
pub fn init(version: Option<&str>, hosts: Option<&str>) -> Result<(), Error> {
    let cwd = std::env::current_dir().map_err(|e| Error::Read(e.to_string()))?;
    let root = crate::runtime::project_root(&cwd)?;

    let running = env!("CARGO_PKG_VERSION");
    let pin_path = root.join(PIN_FILE);
    let pinned = crate::runtime::read_pin(&root)?.map(|version| version.to_string());
    let selected = match version {
        Some(exact) if exact == running => running.to_string(),
        Some(exact) => {
            return Err(Error::Usage(format!(
                "init --version {exact} reached runtime {running}; the dispatcher must delegate to the selected version before init runs"
            )));
        }
        None => pinned.clone().unwrap_or_else(|| running.to_string()),
    };

    // Preflight before any selection default or write: the selected runtime
    // must be cached, every managed JSON setting must parse as plain JSON
    // with a hook structure init understands, and every plugin path must be
    // classifiable without mistaking a read failure for absence.
    let json = preflight_json(&root)?;
    let plugins = preflight_plugins(&root)?;
    let wanted: BTreeSet<&'static str> = match hosts {
        Some(spec) => parse_hosts(spec)?,
        None if interactive() => prompt_hosts(&json, &plugins)?,
        None => existing_hosts(&json, &plugins),
    };
    for slug in ["opencode", "pi", "kilo"] {
        if wanted.contains(slug) && plugins.state(slug) == PluginState::Unmanaged {
            return Err(Error::Refused(format!(
                "{}: not managed by dinyd; move it aside before init",
                host_path(slug)
            )));
        }
    }
    crate::download::fetch_release(
        &crate::download::parse_version(&selected)?,
        crate::download::build_target(),
    )?;

    // An explicit selection that matches the pin, and ordinary setup with an
    // existing pin, both leave the file alone: a fresh clone must not acquire
    // an unrelated version change. (Production dispatch already delegated to
    // the pin, so the running version equals it.)
    let pin_status = if pinned.as_deref() == Some(selected.as_str()) {
        "unchanged"
    } else {
        write_text(&pin_path, &format!("{selected}\n"))?;
        if pinned.is_none() {
            "created"
        } else {
            "updated"
        }
    };

    let mut report: Vec<(String, String, String)> = Vec::new();
    report.push((
        "pin".to_string(),
        PIN_FILE.to_string(),
        format!("{pin_status} {selected}"),
    ));
    for slug in HOST_SLUGS {
        let selected_host = wanted.contains(slug);
        let status = if is_json_host(slug) {
            sync_json_host(&root, &json, slug, selected_host)?
        } else {
            sync_plugin_host(&root, &plugins, slug, selected_host)?
        };
        report.push((slug.to_string(), host_path(slug).to_string(), status));
    }

    println!("dinyd init {selected}");
    for (_, path, status) in &report {
        println!("{path}: {status}");
    }
    println!("PATH: hosts must resolve `dinyd` on PATH; re-run init after moving the binary.");
    println!("Hosts read settings when a session starts; open a new session to activate them.");
    if wanted.contains("codex") {
        println!("Codex: trust the project hooks in /hooks before they run.");
    }
    Ok(())
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

fn host_label(slug: &str) -> &str {
    HOST_SLUGS
        .iter()
        .position(|s| s == &slug)
        .map(|i| HOST_LABELS[i])
        .unwrap_or(slug)
}

/// Parse a `--hosts` value or an interactive answer into a host set.
fn parse_hosts(spec: &str) -> Result<BTreeSet<&'static str>, Error> {
    let mut set = BTreeSet::new();
    let mut none = false;
    for token in spec.split([',', ' ', '\t', '\n']) {
        let token = token.trim().to_lowercase();
        if token.is_empty() {
            continue;
        }
        if token == "none" {
            none = true;
            continue;
        }
        if token == "orca" {
            return Err(Error::Usage(
                "orca has no binding; choose the host of the agent it launches".to_string(),
            ));
        }
        if let Ok(number) = token.parse::<usize>()
            && (1..=HOST_SLUGS.len()).contains(&number)
        {
            set.insert(HOST_SLUGS[number - 1]);
            continue;
        }
        if HOST_SLUGS.contains(&token.as_str()) {
            let index = HOST_SLUGS
                .iter()
                .position(|s| *s == token.as_str())
                .unwrap();
            set.insert(HOST_SLUGS[index]);
            continue;
        }
        return Err(Error::Usage(format!(
            "unknown host `{token}`; choose from {} or `none`",
            HOST_SLUGS.join(", ")
        )));
    }
    if none && !set.is_empty() {
        return Err(Error::Usage(
            "`none` cannot be combined with host names".to_string(),
        ));
    }
    if none {
        set.clear();
    }
    Ok(set)
}

/// Ask which hosts to configure; existing DinyD registrations are preselected.
fn prompt_hosts(
    json: &Preflight,
    plugins: &PluginPreflight,
) -> Result<BTreeSet<&'static str>, Error> {
    let defaults = existing_hosts(json, plugins);
    eprintln!("Configure agent hosts (existing DinyD registrations are preselected):");
    for (index, slug) in HOST_SLUGS.iter().enumerate() {
        let mark = if defaults.contains(slug) { "x" } else { " " };
        eprintln!("  {}) [{mark}] {slug}  {}", index + 1, host_label(slug));
    }
    eprintln!("Enter numbers or slugs separated by commas, `none`, or press Enter to keep.");
    eprint!("hosts> ");
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Error::Read(e.to_string()))?;
    if line.trim().is_empty() {
        return Ok(defaults);
    }
    parse_hosts(&line)
}

/// Hosts that already carry a DinyD registration, from preflight state.
fn existing_hosts(json: &Preflight, plugins: &PluginPreflight) -> BTreeSet<&'static str> {
    let mut set = BTreeSet::new();
    for slug in HOST_SLUGS {
        let owned = if is_json_host(slug) {
            preflight_value(json, slug).is_some_and(|value| has_dinyd_registration(slug, value))
        } else {
            plugins.state(slug) == PluginState::Managed
        };
        if owned {
            set.insert(slug);
        }
    }
    set
}

fn is_json_host(slug: &str) -> bool {
    matches!(slug, "claude-code" | "codex" | "cursor")
}

fn host_path(slug: &str) -> &str {
    match slug {
        "claude-code" => ".claude/settings.json",
        "codex" => ".codex/hooks.json",
        "cursor" => ".cursor/hooks.json",
        "opencode" => ".opencode/plugins/dinyd-guard.ts",
        "pi" => ".pi/extensions/dinyd-guard.ts",
        "kilo" => ".kilo/plugin/dinyd-guard.ts",
        _ => unreachable!("unknown host slug"),
    }
}

fn host_event(slug: &str) -> &str {
    match slug {
        "claude-code" | "codex" => "PreToolUse",
        "cursor" => "preToolUse",
        _ => unreachable!("plugin host has no hook event"),
    }
}

fn host_matcher(slug: &str) -> &str {
    match slug {
        "claude-code" => "Edit|Write|MultiEdit",
        "codex" => "apply_patch",
        "cursor" => "Write|Edit|Delete",
        _ => unreachable!("plugin host has no matcher"),
    }
}

fn guard_command(slug: &str) -> String {
    format!("{GUARD_PREFIX} --host {slug}")
}

fn plugin_template(slug: &str) -> &'static str {
    match slug {
        "opencode" => include_str!("../templates/opencode-dinyd-guard.ts"),
        "pi" => include_str!("../templates/pi-dinyd-guard.ts"),
        "kilo" => include_str!("../templates/kilo-dinyd-guard.ts"),
        _ => unreachable!("json host has no template"),
    }
}

struct Preflight {
    /// Current parsed value per JSON host, in [`HOST_SLUGS`] order.
    values: [Option<serde_json::Value>; 3],
}

/// Read every managed JSON setting before writing anything, and reject hook
/// structures init must not rewrite: a non-object document, a non-object
/// `hooks` table, or a non-array event value.
fn preflight_json(root: &Path) -> Result<Preflight, Error> {
    let claude_code = preflight_one(root, HOST_SLUGS[0])?;
    let codex = preflight_one(root, HOST_SLUGS[1])?;
    let cursor = preflight_one(root, HOST_SLUGS[2])?;
    Ok(Preflight {
        values: [claude_code, codex, cursor],
    })
}

fn preflight_one(root: &Path, slug: &str) -> Result<Option<serde_json::Value>, Error> {
    let path = root.join(host_path(slug));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Read(format!("{}: {e}", host_path(slug)))),
    };
    if text.trim().is_empty() {
        return Ok(Some(serde_json::Value::Object(Default::default())));
    }
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
        Error::Read(format!(
            "{}: not plain JSON; fix or remove it before init",
            host_path(slug)
        ))
    })?;
    validate_hooks(slug, &value)?;
    Ok(Some(value))
}

fn validate_hooks(slug: &str, value: &serde_json::Value) -> Result<(), Error> {
    let rel = host_path(slug);
    let flawed = |what: &str| Error::Read(format!("{rel}: {what}; fix or remove it before init"));
    let document = value
        .as_object()
        .ok_or_else(|| flawed("not a JSON object"))?;
    let Some(hooks) = document.get("hooks") else {
        return Ok(());
    };
    let table = hooks
        .as_object()
        .ok_or_else(|| flawed("`hooks` is not an object"))?;
    for (event, list) in table {
        let entries = list
            .as_array()
            .ok_or_else(|| flawed(&format!("`hooks.{event}` is not an array")))?;
        for (index, entry) in entries.iter().enumerate() {
            let malformed = entry
                .as_object()
                .is_some_and(|group| group.get("hooks").is_some_and(|hooks| !hooks.is_array()));
            if malformed {
                return Err(flawed(&format!(
                    "`hooks.{event}[{index}].hooks` is not an array"
                )));
            }
        }
    }
    Ok(())
}

/// Plugin path state, classified before any write so a read failure never
/// looks like absence.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PluginState {
    Absent,
    Managed,
    Unmanaged,
}

struct PluginPreflight {
    /// States for opencode, pi, and kilo, in that order.
    states: [PluginState; 3],
}

impl PluginPreflight {
    fn state(&self, slug: &str) -> PluginState {
        match slug {
            "opencode" => self.states[0],
            "pi" => self.states[1],
            "kilo" => self.states[2],
            _ => unreachable!("json host has no plugin state"),
        }
    }
}

fn preflight_plugins(root: &Path) -> Result<PluginPreflight, Error> {
    let mut states = [PluginState::Absent; 3];
    for (index, slug) in ["opencode", "pi", "kilo"].iter().enumerate() {
        let rel = host_path(slug);
        match std::fs::read_to_string(root.join(rel)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::Read(format!("{rel}: {e}"))),
            Ok(text) if text.contains(MANAGED_MARKER) => states[index] = PluginState::Managed,
            Ok(_) => states[index] = PluginState::Unmanaged,
        }
    }
    Ok(PluginPreflight { states })
}

fn preflight_value<'p>(preflight: &'p Preflight, slug: &str) -> Option<&'p serde_json::Value> {
    let index = HOST_SLUGS.iter().position(|s| *s == slug).unwrap();
    preflight.values[index].as_ref()
}

fn hook_lists(value: &serde_json::Value) -> Vec<&Vec<serde_json::Value>> {
    let mut lists = Vec::new();
    if let Some(hooks) = value.get("hooks").and_then(|h| h.as_object()) {
        for list in hooks.values() {
            if let Some(list) = list.as_array() {
                lists.push(list);
            }
        }
    }
    lists
}

/// Remove DinyD-owned hooks at any depth, preserving sibling entries.
fn strip_dinyd(entry: &mut serde_json::Value) {
    if let Some(inner) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
        for hook in inner.iter_mut() {
            strip_dinyd(hook);
        }
        inner.retain(|hook| !entry_is_dinyd(hook));
    }
}

fn entry_is_dinyd(entry: &serde_json::Value) -> bool {
    if entry
        .get("command")
        .and_then(|command| command.as_str())
        .is_some_and(|command| command.starts_with(GUARD_PREFIX))
    {
        return true;
    }
    if let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) {
        return hooks.iter().any(entry_is_dinyd);
    }
    false
}

fn has_dinyd_registration(slug: &str, value: &serde_json::Value) -> bool {
    let _ = slug;
    hook_lists(value)
        .iter()
        .any(|list| list.iter().any(entry_is_dinyd))
}

/// The canonical hook entry for a selected host.
fn canonical_entry(slug: &str) -> serde_json::Value {
    let command = guard_command(slug);
    match slug {
        "cursor" => serde_json::json!({
            "command": command,
            "matcher": host_matcher(slug),
            "failClosed": true,
        }),
        _ => serde_json::json!({
            "matcher": host_matcher(slug),
            "hooks": [{"type": "command", "command": command}],
        }),
    }
}

fn canonical_hook(slug: &str) -> serde_json::Value {
    serde_json::json!({"type": "command", "command": guard_command(slug)})
}

/// Merge the desired host state into its JSON value. Returns the value to
/// write, or `None` when the file should not exist. Shapes are preflighted;
/// anything unexpected is left exactly as it was.
fn merged_json(
    slug: &str,
    current: Option<&serde_json::Value>,
    selected: bool,
) -> Option<serde_json::Value> {
    let mut root = current
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
    let map = root.as_object_mut()?;
    let hooks_value = map.shift_remove("hooks");
    let mut hooks = match hooks_value {
        None => Default::default(),
        Some(serde_json::Value::Object(table)) => table,
        Some(other) => {
            map.insert("hooks".to_string(), other);
            return Some(root);
        }
    };

    // Strip every DinyD-owned entry; unrelated entries are preserved as-is.
    // Nested matcher groups lose only their DinyD hooks, so a group that also
    // runs another tool keeps running it. Only events init stripped from are
    // ever pruned; unrelated empty groups stay untouched.
    let mut stripped = BTreeSet::new();
    for (event, list) in hooks.iter_mut() {
        let Some(list) = list.as_array_mut() else {
            continue;
        };
        let before = list.len();
        let owned: Vec<bool> = list.iter().map(entry_is_dinyd).collect();
        for entry in list.iter_mut() {
            strip_dinyd(entry);
        }
        let mut drop: Vec<usize> = Vec::new();
        for (index, entry) in list.iter().enumerate() {
            match entry.get("hooks").and_then(|h| h.as_array()) {
                Some(inner) if inner.is_empty() => {
                    let shell_only = entry
                        .as_object()
                        .is_some_and(|map| map.keys().all(|k| k == "matcher" || k == "hooks"));
                    if shell_only && owned[index] {
                        drop.push(index);
                    }
                }
                Some(_) => {}
                None if entry_is_dinyd(entry) => drop.push(index),
                None => {}
            }
        }
        for index in drop.into_iter().rev() {
            list.remove(index);
        }
        if list.len() != before {
            stripped.insert(event.clone());
        }
    }
    if selected {
        let event = host_event(slug);
        let entry = hooks
            .entry(event.to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        let Some(list) = entry.as_array_mut() else {
            // Preflight rejects this shape; leave the file as it was.
            if !hooks.is_empty() {
                map.insert("hooks".to_string(), serde_json::Value::Object(hooks));
            }
            return Some(root);
        };
        if slug == "cursor" {
            list.push(canonical_entry(slug));
        } else {
            match list
                .iter_mut()
                .find(|e| e.get("matcher").and_then(|m| m.as_str()) == Some(host_matcher(slug)))
            {
                Some(group) => {
                    let map = match group.as_object_mut() {
                        Some(map) => map,
                        // Preflight cannot see this shape; leave the file as it was.
                        None => {
                            if !hooks.is_empty() {
                                map.insert("hooks".to_string(), serde_json::Value::Object(hooks));
                            }
                            return Some(root);
                        }
                    };
                    if !map.get("hooks").is_some_and(|h| h.is_array()) {
                        map.insert("hooks".to_string(), serde_json::Value::Array(Vec::new()));
                    }
                    map.get_mut("hooks")
                        .and_then(|h| h.as_array_mut())
                        .unwrap()
                        .push(canonical_hook(slug));
                }
                None => list.push(canonical_entry(slug)),
            }
        }
    }
    if selected && slug == "cursor" {
        // The host schema versions the file; keep an existing version, add
        // the current one when the file did not name any.
        map.entry("version".to_string())
            .or_insert(serde_json::Value::from(1));
    }
    hooks.retain(|event, list| {
        !stripped.contains(event) || list.as_array().is_none_or(|l| !l.is_empty())
    });
    if !hooks.is_empty() {
        map.insert("hooks".to_string(), serde_json::Value::Object(hooks));
    }

    if map.is_empty() { None } else { Some(root) }
}

fn write_text(path: &Path, text: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Read(format!("{}: {e}", path.display())))?;
    }
    std::fs::write(path, text).map_err(|e| Error::Read(format!("{}: {e}", path.display())))
}

fn render_json(value: &serde_json::Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    text
}

/// Converge one JSON host setting; returns its report status.
fn sync_json_host(
    root: &Path,
    preflight: &Preflight,
    slug: &str,
    selected: bool,
) -> Result<String, Error> {
    let rel = host_path(slug);
    let path = root.join(rel);
    let current = preflight_value(preflight, slug);
    let had_file = std::fs::symlink_metadata(&path).is_ok();
    if !selected && !current.is_some_and(|value| has_dinyd_registration(slug, value)) {
        return Ok("unchanged".to_string());
    }
    let merged = merged_json(slug, current, selected);
    match (had_file, merged) {
        (false, None) => Ok("unchanged".to_string()),
        (true, None) => {
            std::fs::remove_file(&path).map_err(|e| Error::Read(format!("{rel}: {e}")))?;
            Ok("removed".to_string())
        }
        (_, Some(value)) => {
            let text = render_json(&value);
            let unchanged = had_file
                && std::fs::read_to_string(&path)
                    .map(|old| old == text)
                    .unwrap_or(false);
            if unchanged {
                return Ok("unchanged".to_string());
            }
            let created = !had_file;
            write_text(&path, &text)?;
            Ok(if created {
                "created".to_string()
            } else {
                "updated".to_string()
            })
        }
    }
}

/// Converge one plugin adapter file from preflight state; returns its report
/// status. An unmanaged file at a DinyD path is refused in preflight, so a
/// selected host here is always absent or managed.
fn sync_plugin_host(
    root: &Path,
    preflight: &PluginPreflight,
    slug: &str,
    selected: bool,
) -> Result<String, Error> {
    let rel = host_path(slug);
    let path = root.join(rel);
    match (selected, preflight.state(slug)) {
        (false, PluginState::Managed) => {
            std::fs::remove_file(&path).map_err(|e| Error::Read(format!("{rel}: {e}")))?;
            Ok("removed".to_string())
        }
        (false, _) => Ok("unchanged".to_string()),
        (true, PluginState::Absent) => {
            write_text(&path, plugin_template(slug))?;
            Ok("created".to_string())
        }
        (true, PluginState::Managed) => {
            let template = plugin_template(slug);
            let current =
                std::fs::read_to_string(&path).map_err(|e| Error::Read(format!("{rel}: {e}")))?;
            if current == template {
                Ok("unchanged".to_string())
            } else {
                write_text(&path, template)?;
                Ok("updated".to_string())
            }
        }
        (true, PluginState::Unmanaged) => {
            unreachable!("unmanaged selected plugin is refused in preflight")
        }
    }
}
