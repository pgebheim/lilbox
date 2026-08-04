//! Validates `contrib/herdr/herdr-plugin.toml` against herdr's manifest rules
//! so the plugin can't drift out of shape without `cargo test` noticing.
//!
//! The rules mirrored here come from herdr 0.7.x: required top-level keys, id
//! character sets, the event names a `[[events]] on` hook may reference, action
//! contexts, and pane placements. Herdr refuses to link a manifest that breaks
//! any of them, and it does so on the user's machine — cheaper to catch here.

use std::{fs, path::PathBuf};

use toml::Value;

/// `PLUGIN_HOOK_EVENT_KINDS` in herdr — deliberately narrower than the full
/// event set (high-volume events like `pane.output_changed` aren't hookable).
const HOOK_EVENTS: &[&str] = &[
    "workspace.created",
    "workspace.updated",
    "workspace.closed",
    "workspace.renamed",
    "workspace.moved",
    "workspace.reordered",
    "workspace.focused",
    "worktree.created",
    "worktree.opened",
    "worktree.removed",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.moved",
    "tab.focused",
    "pane.created",
    "pane.closed",
    "pane.focused",
    "pane.moved",
    "pane.exited",
    "pane.agent_detected",
    "pane.agent_status_changed",
];

const CONTEXTS: &[&str] = &["global", "workspace", "tab", "pane", "selection"];
const PLACEMENTS: &[&str] = &["overlay", "popup", "split", "tab", "zoomed"];

fn plugin_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contrib/herdr")
}

fn manifest() -> Value {
    let path = plugin_dir().join("herdr-plugin.toml");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn section(manifest: &Value, key: &str) -> Vec<Value> {
    manifest
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn str_at(entry: &Value, key: &str) -> Option<String> {
    entry.get(key)?.as_str().map(str::to_owned)
}

/// argv arrays only — herdr does not run commands through a shell.
fn argv(entry: &Value) -> Vec<String> {
    entry
        .get("command")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().expect("command entries are strings").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn entrypoints(manifest: &Value) -> Vec<Value> {
    ["build", "startup", "actions", "events", "panes"]
        .iter()
        .flat_map(|k| section(manifest, k))
        .collect()
}

#[test]
fn manifest_declares_the_required_top_level_keys() {
    let manifest = manifest();
    for key in ["id", "name", "version", "min_herdr_version"] {
        assert!(
            manifest.get(key).and_then(Value::as_str).is_some(),
            "herdr requires a top-level string `{key}`"
        );
    }
    assert_eq!(manifest["id"].as_str(), Some("lilbox"));
    // microsandbox needs KVM, so the plugin must not advertise macOS/Windows.
    let platforms = manifest["platforms"].as_array().expect("platforms array");
    assert_eq!(
        platforms
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>(),
        ["linux"],
        "lilbox boots libkrun microVMs; only linux can run this plugin"
    );
}

#[test]
fn local_ids_are_unique_and_dot_free() {
    let manifest = manifest();
    // Herdr qualifies action ids as `plugin.id.action`, so a dot in a local id
    // is ambiguous and rejected at link time.
    for kind in ["actions", "panes", "link_handlers"] {
        let mut seen = Vec::new();
        for entry in section(&manifest, kind) {
            let id = str_at(&entry, "id").unwrap_or_else(|| panic!("{kind} entry needs an id"));
            assert!(!id.contains('.'), "{kind} id `{id}` must not contain a dot");
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-')),
                "{kind} id `{id}` has characters herdr rejects"
            );
            assert!(!seen.contains(&id), "duplicate {kind} id `{id}`");
            seen.push(id);
        }
    }
}

#[test]
fn event_hooks_reference_hookable_events() {
    for hook in section(&manifest(), "events") {
        let on = str_at(&hook, "on").expect("event hook needs `on`");
        assert!(
            HOOK_EVENTS.contains(&on.as_str()),
            "`{on}` is not an event a herdr plugin hook can subscribe to"
        );
    }
}

#[test]
fn actions_and_panes_use_valid_contexts_and_placements() {
    let manifest = manifest();
    for action in section(&manifest, "actions") {
        let id = str_at(&action, "id").unwrap();
        for context in action
            .get("contexts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let context = context.as_str().unwrap();
            assert!(
                CONTEXTS.contains(&context),
                "action `{id}` declares unknown context `{context}`"
            );
        }
    }
    for pane in section(&manifest, "panes") {
        let id = str_at(&pane, "id").unwrap();
        if let Some(placement) = str_at(&pane, "placement") {
            assert!(
                PLACEMENTS.contains(&placement.as_str()),
                "pane `{id}` declares unknown placement `{placement}`"
            );
        }
    }
}

#[test]
fn link_handlers_point_at_actions_this_plugin_declares() {
    let manifest = manifest();
    let actions: Vec<String> = section(&manifest, "actions")
        .iter()
        .filter_map(|a| str_at(a, "id"))
        .collect();
    for handler in section(&manifest, "link_handlers") {
        let id = str_at(&handler, "id").unwrap();
        let action = str_at(&handler, "action")
            .unwrap_or_else(|| panic!("link handler `{id}` needs an action"));
        assert!(
            actions.contains(&action),
            "link handler `{id}` points at `{action}`, which this plugin does not declare"
        );
        assert!(
            str_at(&handler, "pattern").is_some_and(|p| !p.is_empty()),
            "link handler `{id}` needs a pattern"
        );
    }
}

#[test]
fn shim_commands_exist_and_are_executable() {
    let dir = plugin_dir();
    for entry in entrypoints(&manifest()) {
        let argv = argv(&entry);
        let Some(program) = argv.first() else {
            continue;
        };
        // Relative programs resolve against the plugin dir, which herdr uses as
        // the working directory for plugin commands.
        if !program.contains('/') {
            continue;
        }
        let path = dir.join(program);
        assert!(
            path.is_file(),
            "manifest runs `{program}`, which is missing"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert!(
                mode & 0o111 != 0,
                "`{program}` is not executable (mode {mode:o}); herdr execs it directly"
            );
        }
    }
}

/// The manifest and the shim's dispatch table are two halves of one contract;
/// a renamed subcommand on either side is a silent runtime failure otherwise.
#[test]
fn every_shim_subcommand_in_the_manifest_is_dispatched() {
    let shim = fs::read_to_string(plugin_dir().join("bin/lilbox-herdr")).unwrap();
    let dispatch = shim.split_once("main() {").expect("shim has a main()").1;
    for entry in entrypoints(&manifest()) {
        let argv = argv(&entry);
        if argv.first().is_none_or(|p| !p.ends_with("lilbox-herdr")) {
            continue;
        }
        let subcommand = argv.get(1).expect("shim invoked without a subcommand");
        assert!(
            dispatch.contains(&format!("{subcommand})")),
            "manifest calls `lilbox-herdr {subcommand}`, which main() does not dispatch"
        );
    }
}
