use anyhow::{Context, Result, anyhow, bail};
use chrono::Local;
use microsandbox::{MicrosandboxError, Sandbox, Volume, sandbox::SandboxStatus};
use rusqlite::params;

use super::net;
use crate::app::App;
use crate::provision::{provision, teardown};
use crate::sandbox::{SandboxSettings, configure_builder, stop_and_remove};
use crate::tailscale::{ForkPlan, TAILNET_SCRUB_SCRIPT, plan_fork};
use crate::util::{DEFAULT_GUEST_PORT, alloc_host_port, now};

pub(crate) async fn gc(app: &App) -> Result<()> {
    let expired: Vec<_> = app
        .rows()?
        .into_iter()
        .filter(|r| r.expires.is_some_and(|t| t < now()))
        .collect();
    if expired.is_empty() {
        println!("nothing to gc - no expired boxes");
    }
    for row in expired {
        match teardown(app, &row).await {
            Ok(()) => println!("reaped {} (TTL elapsed)", row.name),
            Err(err) => eprintln!("warning: could not reap '{}': {err:#}", row.name),
        }
    }
    Ok(())
}

pub(crate) async fn provision_cmd(app: &App, name: String) -> Result<()> {
    let row = app.require_row(&name)?;
    let template_name = row
        .template
        .ok_or_else(|| anyhow!("box '{name}' was not created from a template"))?;
    provision(app, &name, &app.template(&template_name)?).await?;
    Ok(())
}

pub(crate) async fn stop(app: &App, name: String) -> Result<()> {
    app.require_row(&name)?;
    match Sandbox::get(&name).await?.stop().await {
        Ok(()) | Err(MicrosandboxError::SandboxNotRunning(_)) => {}
        Err(err) => return Err(err.into()),
    }
    app.db.execute(
        "UPDATE boxes SET stopped_reason='user' WHERE name=?1",
        [&name],
    )?;
    println!("stopped {name}");
    Ok(())
}

pub(crate) async fn start(app: &App, name: String) -> Result<()> {
    app.require_row(&name)?;
    Sandbox::get(&name).await?.start_detached().await?;
    app.db.execute(
        "UPDATE boxes SET stopped_reason=NULL WHERE name=?1",
        [&name],
    )?;
    println!("started {name}");
    Ok(())
}

pub(crate) async fn restart(app: &App, name: String) -> Result<()> {
    app.require_row(&name)?;
    let handle = Sandbox::get(&name).await?;
    match handle.stop().await {
        Ok(()) | Err(MicrosandboxError::SandboxNotRunning(_)) => {}
        Err(err) => return Err(err.into()),
    }
    Sandbox::start_detached(&name).await?;
    app.db.execute(
        "UPDATE boxes SET stopped_reason=NULL WHERE name=?1",
        [&name],
    )?;
    println!("restarted {name}");
    Ok(())
}

pub(crate) async fn rm(app: &App, name: String, keep_data: bool) -> Result<()> {
    let row = app.require_row(&name)?;
    teardown(app, &row).await?;
    if let Some(volume) = row.volume {
        let shared: bool = app.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM boxes WHERE volume=?1)",
            [&volume],
            |r| r.get(0),
        )?;
        if keep_data || shared {
            println!("removed {name} - kept volume {volume}");
        } else {
            if let Err(err) = Volume::remove(&volume).await
                && !matches!(err, MicrosandboxError::VolumeNotFound(_))
            {
                eprintln!("warning: could not remove volume {volume}: {err}");
            }
            println!("removed {name} (and its home volume)");
        }
    } else {
        println!("removed {name}");
    }
    Ok(())
}

pub(crate) async fn fork(
    app: &App,
    name: String,
    newname: Option<String>,
    force: bool,
) -> Result<()> {
    let row = app.require_row(&name)?;
    let new = newname.unwrap_or_else(|| format!("{name}-fork"));
    if app.row(&new)?.is_some() {
        bail!("box '{new}' already exists");
    }
    // After the free checks (both pure DB reads) but before any snapshot work:
    // --force mutates the parent by taking it off the tailnet, so everything
    // that can cheaply fail must fail first. Running here also keeps the guard
    // testable without KVM.
    match plan_fork(row.tailscale_node.as_deref(), force) {
        ForkPlan::Snapshot => {}
        ForkPlan::LeaveThenSnapshot => {
            println!("leaving the tailnet before snapshotting {name} ...");
            net::leave(app, name.clone()).await?;
        }
        ForkPlan::Refuse => bail!(
            "box '{name}' is on the tailnet — a snapshot would clone its node identity.\n\
             Run 'lilbox leave {name}' first, or pass --force to leave and fork in one step."
        ),
    }
    let handle = Sandbox::get(&name).await?;
    let was_running = matches!(
        handle.status_snapshot(),
        SandboxStatus::Running | SandboxStatus::Draining
    );
    handle.stop().await?;
    let snapshot = format!("lilbox-{name}-{}", now());
    let snapshot_result = handle.snapshot(&snapshot).await;
    if was_running {
        let restart_result = Sandbox::start_detached(&name).await;
        match (snapshot_result, restart_result) {
            (Ok(_), Ok(_)) => {}
            (Err(snapshot_error), Ok(_)) => return Err(snapshot_error.into()),
            (Ok(_), Err(restart_error)) => {
                return Err(restart_error)
                    .with_context(|| format!("snapshot created but could not restart '{name}'"));
            }
            (Err(snapshot_error), Err(restart_error)) => {
                bail!(
                    "snapshot failed: {snapshot_error}; additionally could not restart '{name}': {restart_error}"
                );
            }
        }
    } else {
        snapshot_result?;
    }
    let host_port = alloc_host_port()?;
    let guest_port = row.guest_port.unwrap_or(DEFAULT_GUEST_PORT);
    Sandbox::builder(&new)
        .from_snapshot(&snapshot)
        .port(host_port, guest_port)
        .detached(true)
        .create_detached()
        .await?;
    // A NULL tailscale_node doesn't prove the guest is clean: a box joined by
    // hand (`lilbox exec … tailscale up`) leaves state lilbox never recorded, so
    // the guard above can't see it. Scrub unconditionally -- the guard covers
    // what lilbox knows about, this covers the rest.
    scrub_tailnet_state(&new).await;
    app.db.execute("INSERT INTO boxes(name,image,guest_port,host_port,created,comment) VALUES(?1,?2,?3,?4,?5,?6)",
        params![new, row.image, row.guest_port, host_port, Local::now().format("%Y-%m-%d %H:%M:%S").to_string(), format!("forked from {name}")])?;
    println!("forked {name} -> {new}");
    Ok(())
}

/// Best-effort: strip any tailnet identity the snapshot carried into a clone.
/// Failures only warn — an un-scrubbed clone is still a usable box, and this
/// must never fail a fork that has already booted the VM.
async fn scrub_tailnet_state(name: &str) {
    let result: Result<()> = async {
        let handle = Sandbox::get(name).await?;
        match handle.status_snapshot() {
            SandboxStatus::Running | SandboxStatus::Draining => {}
            _ => return Ok(()),
        }
        let sandbox = handle.connect().await?;
        let output = sandbox
            .exec("/bin/sh", ["-c", TAILNET_SCRUB_SCRIPT])
            .await?;
        if !output.status().success {
            bail!("scrub script exited non-zero");
        }
        Ok(())
    }
    .await;
    if let Err(err) = result {
        eprintln!("warning: could not clear inherited tailnet state in '{name}': {err:#}");
    }
}

pub(crate) async fn rebuild(app: &App, name: String, image: Option<String>) -> Result<()> {
    let row = app.require_row(&name)?;
    let volume = row
        .volume
        .as_deref()
        .ok_or_else(|| anyhow!("box '{name}' has no persistent volume"))?;
    let image = image.unwrap_or(row.image);
    let host_port = row
        .host_port
        .ok_or_else(|| anyhow!("box has no host port"))?;
    stop_and_remove(&name).await?;
    configure_builder(SandboxSettings {
        name: &name,
        image: &image,
        host_port,
        guest_port: row.guest_port.unwrap_or(DEFAULT_GUEST_PORT),
        cpus: None,
        memory: None,
        idle_timeout: None,
        volume: Some(volume),
    })
    .create_detached()
    .await?;
    app.db.execute(
        "UPDATE boxes SET image=?1 WHERE name=?2",
        params![image, name],
    )?;
    println!("rebuilt {name} on {image} (home data intact)");
    Ok(())
}
