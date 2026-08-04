use anyhow::{Result, anyhow};
use rusqlite::params;

use crate::app::App;
use crate::tailscale::{allocate_serve_port, tailnet_host};
use crate::util::{run_external, successful_output};

pub(crate) fn expose(app: &App, name: String, public: bool) -> Result<()> {
    let ts = app
        .tailscale
        .as_deref()
        .ok_or_else(|| anyhow!("tailscale not found - cannot publish"))?;
    let row = app.require_row(&name)?;
    if let Some(url) = row.url {
        println!("already exposed at {url}");
        return Ok(());
    }
    let host_port = row
        .host_port
        .ok_or_else(|| anyhow!("box '{name}' has no published port"))?;
    let port = allocate_serve_port(app, public)?;
    let verb = if public { "funnel" } else { "serve" };
    successful_output(
        ts,
        &[
            verb,
            "--bg",
            &format!("--https={port}"),
            &format!("http://127.0.0.1:{host_port}"),
        ],
    )?;
    let host = tailnet_host(ts).unwrap_or_else(|| "this-host".into());
    let suffix = if public && port == 443 {
        String::new()
    } else {
        format!(":{port}")
    };
    let url = format!("https://{host}{suffix}/");
    app.db.execute(
        "UPDATE boxes SET serve_port=?1,public=?2,url=?3 WHERE name=?4",
        params![port, public as i32, url, name],
    )?;
    println!(
        "{name} is live {}:\n    {url}",
        if public {
            "publicly"
        } else {
            "on your tailnet"
        }
    );
    Ok(())
}

pub(crate) fn unexpose(app: &App, name: String) -> Result<()> {
    let row = app.require_row(&name)?;
    let Some(port) = row.serve_port else {
        println!("'{name}' is not exposed");
        return Ok(());
    };
    if let Some(ts) = &app.tailscale {
        let verb = if row.public { "funnel" } else { "serve" };
        let _ = run_external(ts, &[verb, &format!("--https={port}"), "off"]);
    }
    app.db.execute(
        "UPDATE boxes SET serve_port=NULL,public=0,url=NULL WHERE name=?1",
        [&name],
    )?;
    println!("unpublished {name}");
    Ok(())
}

pub(crate) fn url(app: &App, name: String) -> Result<()> {
    println!(
        "{}",
        app.require_row(&name)?
            .url
            .ok_or_else(|| anyhow!("'{name}' is not exposed (vm expose {name})"))?
    );
    Ok(())
}
