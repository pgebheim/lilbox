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

/// The `lilbox-herdr` subcommand an entrypoint runs, for either command form:
/// the shim invoked directly (`["bin/lilbox-herdr", "<sub>"]`, used by actions)
/// or through a launcher that resolves it by absolute path (`["sh", "-c", "exec
/// \"$HERDR_PLUGIN_ROOT/bin/lilbox-herdr\" <sub>"]`, used by panes — herdr's pane
/// spawner PATH-searches argv[0], so a relative program never resolves). Returns
/// `None` for entrypoints that don't run the shim (e.g. the `herdr plugin pane
/// open` actions).
fn shim_subcommand(argv: &[String]) -> Option<String> {
    if argv.first().is_some_and(|p| p.ends_with("lilbox-herdr")) {
        return argv.get(1).cloned();
    }
    let inner = argv.iter().find(|a| a.contains("lilbox-herdr"))?;
    inner.split_whitespace().last().map(str::to_owned)
}

/// Plugin-dir-relative paths to any bundled executable an entrypoint runs, for
/// either command form. A direct `bin/…` token, or a `$HERDR_PLUGIN_ROOT/…`
/// target inside a launcher string (both `$VAR/` and `${VAR}/` are matched).
/// Programs on PATH (`herdr`, `sh`) yield nothing — they aren't files we ship.
fn plugin_relative_targets(argv: &[String]) -> Vec<String> {
    const ROOT: &str = "HERDR_PLUGIN_ROOT";
    let mut out = Vec::new();
    for a in argv {
        if a.starts_with("bin/") {
            out.push(a.clone());
        }
        if let Some(idx) = a.find(ROOT) {
            // Tolerate the `${HERDR_PLUGIN_ROOT}/…` brace form, then the `/`.
            let after = a[idx + ROOT.len()..]
                .strip_prefix('}')
                .unwrap_or(&a[idx + ROOT.len()..]);
            if let Some(rest) = after.strip_prefix('/') {
                let rel: String = rest
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != '"')
                    .collect();
                if !rel.is_empty() {
                    out.push(rel);
                }
            }
        }
    }
    out
}

/// The program a pane will actually execute: argv[0] directly, or — when argv[0]
/// is a shell launcher — the token it `exec`s. Lets the resolvability check below
/// see the *real* program (`$HERDR_PLUGIN_ROOT/bin/lilbox-herdr`), not just `sh`.
fn effective_pane_program(argv: &[String]) -> Option<String> {
    let first = argv.first()?;
    if matches!(first.as_str(), "sh" | "bash" | "dash") {
        let script = argv.iter().find(|a| a.contains("exec "))?;
        let after = script.split("exec ").nth(1)?.trim_start();
        return Some(
            after
                .chars()
                .skip_while(|c| *c == '"')
                .take_while(|c| !c.is_whitespace() && *c != '"')
                .collect(),
        );
    }
    Some(first.clone())
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

/// Herdr's *pane* spawner resolves a pane command's program (argv[0]) via PATH —
/// unlike the action runner, it does not honour the plugin dir as the program's
/// lookup root. A relative path like `bin/lilbox-herdr` is "not found in PATH"
/// and the pane fails to open (`plugin_pane_open_failed`), which silently breaks
/// the plugin's headline box/agent panes. So a pane program must be a bare PATH
/// command (e.g. `sh`) or absolute; the shim is reached by absolute path through
/// `$HERDR_PLUGIN_ROOT` from inside that launcher.
#[test]
fn pane_programs_are_path_resolvable() {
    for pane in section(&manifest(), "panes") {
        let id = str_at(&pane, "id").unwrap();
        let program = effective_pane_program(&argv(&pane))
            .unwrap_or_else(|| panic!("pane `{id}` has no runnable program"));
        // A bare PATH command (`sh`), an absolute path, or a `$HERDR_PLUGIN_ROOT`-
        // rooted target all resolve. A relative path with a slash does not — and
        // that's true whether it's argv[0] or the program the launcher execs, so
        // we check the *effective* program, not just argv[0].
        let resolvable =
            !program.contains('/') || program.starts_with('/') || program.starts_with('$');
        assert!(
            resolvable,
            "pane `{id}` runs `{program}`: herdr's pane spawner PATH-searches the program, \
             so a relative path never resolves. Use a bare command, an absolute path, or one \
             rooted at $HERDR_PLUGIN_ROOT (e.g. `sh -c 'exec \"$HERDR_PLUGIN_ROOT/bin/lilbox-herdr\" {id}'`)."
        );
    }
}

#[test]
fn shim_commands_exist_and_are_executable() {
    let dir = plugin_dir();
    for entry in entrypoints(&manifest()) {
        // Verify every bundled executable an entrypoint runs, whether named
        // directly (`bin/lilbox-herdr`, actions) or by absolute path via
        // `$HERDR_PLUGIN_ROOT` (panes). A typo in either form makes the path
        // vanish and this fail.
        for rel in plugin_relative_targets(&argv(&entry)) {
            let path = dir.join(&rel);
            assert!(path.is_file(), "manifest runs `{rel}`, which is missing");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&path).unwrap().permissions().mode();
                assert!(
                    mode & 0o111 != 0,
                    "`{rel}` is not executable (mode {mode:o}); herdr execs it directly"
                );
            }
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
        let Some(subcommand) = shim_subcommand(&argv(&entry)) else {
            continue;
        };
        assert!(
            dispatch.contains(&format!("{subcommand})")),
            "manifest calls `lilbox-herdr {subcommand}`, which main() does not dispatch"
        );
    }
}
