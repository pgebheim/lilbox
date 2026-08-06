use std::{
    env,
    net::TcpListener,
    path::Path,
    process::Command as ProcessCommand,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rand::Rng;

use crate::app::App;

pub(crate) const DEFAULT_IMAGE: &str = "python";
pub(crate) const DEFAULT_GUEST_PORT: u16 = 8000;

pub(crate) fn find_program(name: &str) -> Option<std::path::PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.is_file())
    })
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Generate a box name not already claimed in the state DB. Retries a
/// bounded number of times on collision, mirroring the shell's loop.
/// Note: unlike the shell, this does not additionally check live sandbox
/// names (that requires an async `statuses()` call not available from this
/// helper's callers without extra plumbing); DB-uniqueness is checked instead.
pub(crate) fn random_name(app: &App) -> Result<String> {
    for _ in 0..100 {
        let candidate = format!("box-{:06x}", rand::rng().random_range(0..=0xff_ffff_u32));
        if app.row(&candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    bail!("could not generate a unique box name")
}

pub(crate) fn alloc_host_port() -> Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

pub(crate) fn parse_duration(value: &str) -> Result<u64> {
    let (number, multiplier) = match value.chars().last() {
        Some('s') => (&value[..value.len() - 1], 1),
        Some('m') => (&value[..value.len() - 1], 60),
        Some('h') => (&value[..value.len() - 1], 3600),
        Some('d') => (&value[..value.len() - 1], 86400),
        _ => (value, 1),
    };
    number
        .parse::<u64>()
        .map(|n| n * multiplier)
        .with_context(|| format!("invalid duration '{value}' (use e.g. 30s, 5m, 2h, 1d)"))
}

pub(crate) fn parse_memory(value: &str) -> Result<u32> {
    let upper = value.trim().to_ascii_uppercase();
    let (number, multiplier) = match upper.chars().last() {
        Some('M') => (&upper[..upper.len() - 1], 1.0),
        Some('G') => (&upper[..upper.len() - 1], 1024.0),
        Some('T') => (&upper[..upper.len() - 1], 1024.0 * 1024.0),
        _ => (upper.as_str(), 1.0),
    };
    let mib = number
        .parse::<f64>()
        .with_context(|| format!("invalid memory size '{value}'"))?
        * multiplier;
    if !(1.0..=u32::MAX as f64).contains(&mib) {
        bail!("invalid memory size '{value}'");
    }
    Ok(mib as u32)
}

pub(crate) fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    let mut value = bytes as f64;
    for unit in ["K", "M", "G", "T"] {
        value /= 1024.0;
        if value < 1024.0 || unit == "T" {
            return format!("{value:.1}{unit}");
        }
    }
    unreachable!()
}

pub(crate) fn run_external(program: &Path, args: &[&str]) -> Result<std::process::Output> {
    ProcessCommand::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {}", program.display()))
}

pub(crate) fn successful_output(program: &Path, args: &[&str]) -> Result<String> {
    let output = run_external(program, args)?;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run `result`; if it's `Err`, run `cleanup` exactly once (best-effort — a
/// cleanup failure is swallowed, the ORIGINAL error is returned). On `Ok`,
/// `cleanup` never runs. Returns the original result. Shared by the atomic
/// box-provisioning paths (`commands::new`, `commands::agent`) so a failure or
/// interrupt after the VM is built tears it down instead of orphaning it.
pub(crate) async fn or_cleanup<T, C, Fut>(result: Result<T>, cleanup: C) -> Result<T>
where
    C: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            let _ = cleanup().await;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30").unwrap(), 30);
        assert_eq!(parse_duration("5m").unwrap(), 300);
        assert_eq!(parse_duration("2h").unwrap(), 7200);
        assert!(parse_duration("soon").is_err());
    }

    #[test]
    fn parses_memory_as_mib() {
        assert_eq!(parse_memory("512M").unwrap(), 512);
        assert_eq!(parse_memory("2G").unwrap(), 2048);
        assert!(parse_memory("lots").is_err());
    }

    #[tokio::test]
    async fn or_cleanup_ok_result_does_not_run_cleanup() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = AtomicUsize::new(0);
        let result: Result<i32> = Ok(42);

        let outcome = or_cleanup(result, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;

        assert_eq!(outcome.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn or_cleanup_err_result_runs_cleanup_once_and_returns_original_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = AtomicUsize::new(0);
        let result: Result<i32> = Err(anyhow::anyhow!("original failure"));

        let outcome = or_cleanup(result, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;

        assert_eq!(outcome.unwrap_err().to_string(), "original failure");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn or_cleanup_err_result_swallows_cleanup_error_and_returns_original_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = AtomicUsize::new(0);
        let result: Result<i32> = Err(anyhow::anyhow!("original failure"));

        let outcome = or_cleanup(result, || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!("cleanup failure"))
        })
        .await;

        assert_eq!(outcome.unwrap_err().to_string(), "original failure");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
