use anyhow::{Result, anyhow, bail};
use chrono::Local;
use microsandbox::{MicrosandboxError, Sandbox, Volume, sandbox::SandboxStatus};
use rusqlite::params;

use crate::app::App;
use crate::provision::{provision, teardown};
use crate::sandbox::{SandboxSettings, configure_builder, stop_and_remove};
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

pub(crate) async fn fork(app: &App, name: String, newname: Option<String>) -> Result<()> {
    let row = app.require_row(&name)?;
    let new = newname.unwrap_or_else(|| format!("{name}-fork"));
    if app.row(&new)?.is_some() {
        bail!("box '{new}' already exists");
    }
    let handle = Sandbox::get(&name).await?;
    let was_running = matches!(
        handle.status_snapshot(),
        SandboxStatus::Running | SandboxStatus::Draining
    );
    handle.stop().await?;
    let snapshot = format!("lilexe-{name}-{}", now());
    handle.snapshot(&snapshot).await?;
    if was_running {
        Sandbox::start_detached(&name).await?;
    }
    let host_port = alloc_host_port()?;
    let guest_port = row.guest_port.unwrap_or(DEFAULT_GUEST_PORT);
    Sandbox::builder(&new)
        .from_snapshot(&snapshot)
        .port(host_port, guest_port)
        .detached(true)
        .create_detached()
        .await?;
    app.db.execute("INSERT INTO boxes(name,image,guest_port,host_port,created,comment) VALUES(?1,?2,?3,?4,?5,?6)",
        params![new, row.image, row.guest_port, host_port, Local::now().format("%Y-%m-%d %H:%M:%S").to_string(), format!("forked from {name}")])?;
    println!("forked {name} -> {new}");
    Ok(())
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
