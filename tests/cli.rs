use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_home(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("lilexe-{label}-{}-{nonce}", std::process::id()))
}

#[test]
fn help_does_not_initialize_user_state() {
    let home = temp_home("help");
    let output = Command::new(env!("CARGO_BIN_EXE_vm"))
        .arg("--help")
        .env("HOME", &home)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!home.join(".lilexe").exists());
    let _ = fs::remove_dir_all(home);
}

#[test]
fn parse_errors_do_not_initialize_user_state() {
    let home = temp_home("parse-error");
    let output = Command::new(env!("CARGO_BIN_EXE_vm"))
        .arg("not-a-command")
        .env("HOME", &home)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!home.join(".lilexe").exists());
    let _ = fs::remove_dir_all(home);
}
