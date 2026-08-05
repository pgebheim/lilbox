use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_home(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("lilbox-{label}-{}-{nonce}", std::process::id()))
}

/// Build a `lilbox` invocation pinned to `home`, with the XDG_* overrides
/// cleared so `dirs` resolves state strictly under `home` (an inherited
/// `XDG_CONFIG_HOME` on the runner would otherwise send state outside it,
/// escaping the no-state-init guards below).
fn lilbox_cmd(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lilbox"));
    cmd.env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_DATA_HOME");
    cmd
}

/// Assert `lilbox` initialized no on-disk state under `home`. `App::new()`
/// writes to the XDG dirs — `~/.config/lilbox`, `~/.local/state/lilbox`,
/// `~/.local/share/lilbox` — not the legacy `~/.lilbox`, so checking those is
/// what actually proves a command short-circuited before app init.
fn assert_no_lilbox_state(home: &Path) {
    for rel in [
        ".config/lilbox",
        ".local/state/lilbox",
        ".local/share/lilbox",
        ".lilbox",
    ] {
        assert!(
            !home.join(rel).exists(),
            "lilbox initialized state at {rel}"
        );
    }
}

/// `lilbox` must not create user state just because it was asked a question.
/// `App::new()` writes the XDG dirs, so a command that only prints text has to
/// short-circuit before app init.
mod initializes_no_state {
    use super::*;

    #[test]
    fn on_help() {
        let home = temp_home("help");
        let output = Command::new(env!("CARGO_BIN_EXE_lilbox"))
            .arg("--help")
            .env("HOME", &home)
            .output()
            .unwrap();

        assert!(output.status.success());
        assert!(!home.join(".lilbox").exists());
        let _ = fs::remove_dir_all(home);
    }

    /// An unparseable command line fails before any state is touched.
    #[test]
    fn on_parse_error() {
        let home = temp_home("parse-error");
        let output = Command::new(env!("CARGO_BIN_EXE_lilbox"))
            .arg("not-a-command")
            .env("HOME", &home)
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert!(!home.join(".lilbox").exists());
        let _ = fs::remove_dir_all(home);
    }

    /// Shell completions are generated at install time, often before the user
    /// has any lilbox state -- generating them must not create it.
    #[test]
    fn on_completions() {
        let home = temp_home("completions-state");
        let output = lilbox_cmd(&home)
            .args(["completions", "bash"])
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_no_lilbox_state(&home);
        let _ = fs::remove_dir_all(home);
    }
}

mod completions {
    use super::*;

    #[test]
    fn prints_per_shell() {
        // Markers are clap_complete v4's per-shell template idioms — if a future
        // clap_complete bump changes its generated boilerplate, these are the
        // strings to re-check.
        let cases = [
            ("bash", "complete"),
            ("zsh", "#compdef"),
            ("fish", "complete -c lilbox"),
            ("elvish", "edit:completion"),
            ("powershell", "Register-ArgumentCompleter"),
        ];

        for (shell, marker) in cases {
            let home = temp_home(&format!("completions-{shell}"));
            let output = lilbox_cmd(&home)
                .args(["completions", shell])
                .output()
                .unwrap();

            assert!(output.status.success(), "shell {shell} did not succeed");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(!stdout.is_empty(), "shell {shell} produced empty stdout");
            assert!(
                stdout.contains("lilbox"),
                "shell {shell} stdout missing binary name: {stdout}"
            );
            assert!(
                stdout.contains(marker),
                "shell {shell} stdout missing marker {marker:?}: {stdout}"
            );
            let _ = fs::remove_dir_all(home);
        }
    }

    /// An unknown shell fails and prints nothing -- no partial script that a
    /// caller might source.
    #[test]
    fn rejects_invalid_shell() {
        let home = temp_home("completions-invalid-shell");
        let output = lilbox_cmd(&home)
            .args(["completions", "notashell"])
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_no_lilbox_state(&home);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn requires_shell_arg() {
        let home = temp_home("completions-missing-shell");
        let output = lilbox_cmd(&home).arg("completions").output().unwrap();

        assert!(!output.status.success());
        assert_no_lilbox_state(&home);
        let _ = fs::remove_dir_all(home);
    }

    /// Discoverable from `--help`, or nobody knows to run it.
    #[test]
    fn appears_in_help() {
        let home = temp_home("completions-help");
        let output = lilbox_cmd(&home).arg("--help").output().unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("completions"));
        let _ = fs::remove_dir_all(home);
    }
}
