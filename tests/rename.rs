//! Pins the project rename lilexe -> lilbox (issue #64).
//!
//! These tests read files straight off disk under `CARGO_MANIFEST_DIR` so
//! they fail (rather than fail to compile) while the rename hasn't happened
//! yet. Once the rename lands, they should pass with no further edits here.

use std::fs;
use std::path::{Path, PathBuf};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Recursively collect every file under `dir`, skipping `target/` build
/// output directories. Returns an empty vec if `dir` doesn't exist.
fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" {
                continue;
            }
            collect_files_recursive(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Append a `path:line` entry for every line containing a case-insensitive
/// occurrence of "lilexe". The repo has been renamed to lilbox, so there is
/// no longer any allowed exception.
fn find_lilexe_offenses(path: &Path, content: &str, offenses: &mut Vec<String>) {
    for (idx, line) in content.lines().enumerate() {
        if line.to_lowercase().contains("lilexe") {
            offenses.push(format!("{}:{}", path.display(), idx + 1));
        }
    }
}

#[test]
fn no_legacy_lilexe_identifiers() {
    let root = manifest_dir();
    let mut files: Vec<PathBuf> = Vec::new();

    // src/ — all *.rs, recursive
    let src_dir = root.join("src");
    if src_dir.exists() {
        let mut src_files = Vec::new();
        collect_files_recursive(&src_dir, &mut src_files);
        files.extend(
            src_files
                .into_iter()
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("rs")),
        );
    }

    // Individual top-level files.
    for rel in [
        "Cargo.toml",
        "README.md",
        "config.toml.example",
        ".gitignore",
        "install.sh",
        ".github/workflows/ci.yml",
        "tests/cli.rs",
    ] {
        let p = root.join(rel);
        if p.exists() {
            files.push(p);
        }
    }

    let rig_config = root.join(".rig").join("config.json");
    if rig_config.exists() {
        files.push(rig_config);
    }

    // images/ — recursive, text files (binary/non-UTF-8 files are skipped
    // below when we fail to read them as a string).
    let images_dir = root.join("images");
    if images_dir.exists() {
        collect_files_recursive(&images_dir, &mut files);
    }

    let mut offenses: Vec<String> = Vec::new();

    for path in &files {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // not valid UTF-8 (or unreadable) — skip
        };
        find_lilexe_offenses(path, &content, &mut offenses);
    }

    assert!(
        offenses.is_empty(),
        "found legacy 'lilexe' identifiers that should have been renamed to 'lilbox':\n{}",
        offenses.join("\n")
    );
}

/// Very small, purpose-built TOML reader: does `content` declare a
/// `[[bin]]` table whose `name` equals `expected`? Returns the set of bin
/// names actually found, for a useful failure message.
fn bin_names(content: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_bin_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("[[") {
            in_bin_section = trimmed == "[[bin]]";
            continue;
        }
        if trimmed.starts_with('[') {
            in_bin_section = false;
            continue;
        }

        if in_bin_section && let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                let name = value.trim().trim_matches('"').to_string();
                names.push(name);
            }
        }
    }

    names
}

#[test]
fn binary_is_named_lilbox() {
    let root = manifest_dir();
    let cargo_toml = root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml).expect("Cargo.toml should be readable UTF-8");

    let names = bin_names(&content);

    assert!(
        names.iter().any(|n| n == "lilbox"),
        "expected a [[bin]] with name = \"lilbox\" in Cargo.toml, found bin name(s): {:?}",
        names
    );
}

#[test]
fn rename_target_is_lilbox() {
    let root = manifest_dir();
    let src_dir = root.join("src");

    let mut src_files = Vec::new();
    collect_files_recursive(&src_dir, &mut src_files);
    let src_files: Vec<PathBuf> = src_files
        .into_iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("rs"))
        .collect();

    assert!(
        !src_files.is_empty(),
        "expected at least one .rs file under src/ to check for 'lilbox'"
    );

    let found = src_files.iter().any(|path| match fs::read_to_string(path) {
        Ok(content) => content.to_lowercase().contains("lilbox"),
        Err(_) => false,
    });

    assert!(
        found,
        "expected at least one file under src/ to mention 'lilbox' \
         (the rename should introduce the new name, not just delete the old one)"
    );
}
