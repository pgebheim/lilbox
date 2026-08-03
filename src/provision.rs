use std::{env, fs, process::Command as ProcessCommand};

use anyhow::{Result, anyhow, bail};
use microsandbox::Image;

use crate::app::App;
use crate::model::{BoxRow, Template};
use crate::sandbox::{connect_box, stop_and_remove};
use crate::util::{find_program, now, run_external};

pub(crate) async fn provision(app: &App, name: &str, template: &Template) -> Result<()> {
    let Some(setup) = &template.setup else {
        return Ok(());
    };
    let sandbox = connect_box(app, name, true).await?;
    fs::create_dir_all(app.state_dir.join("logs"))?;
    sandbox.fs().write("/tmp/lilexe-setup.sh", setup).await?;
    println!("provisioning {name} (template {}) ...", template.name);
    let output = sandbox.exec("/bin/sh", ["/tmp/lilexe-setup.sh"]).await?;
    let mut combined = output.stdout_bytes().to_vec();
    combined.extend_from_slice(output.stderr_bytes());
    let log = app.state_dir.join("logs").join(format!("{name}-setup.log"));
    fs::write(&log, combined)?;
    if !output.status().success {
        bail!("setup failed for '{name}' (full log: {})", log.display());
    }
    println!("provisioned {name} (log: {})", log.display());
    Ok(())
}

pub(crate) async fn build_template_image(template: &Template, rebuild: bool) -> Result<String> {
    let tag = format!("lilexe/{}:latest", template.name);
    if !rebuild && Image::get(&tag).await.is_ok() {
        return Ok(tag);
    }
    let dir = template
        .dir
        .as_ref()
        .ok_or_else(|| anyhow!("built-in template has no Dockerfile directory"))?;
    let docker = find_program("docker")
        .ok_or_else(|| anyhow!("docker not found (needed to build template image)"))?;
    let status = ProcessCommand::new(&docker)
        .args(["build", "-t", &tag])
        .arg(dir)
        .status()?;
    if !status.success() {
        bail!("docker build failed for template '{}'", template.name);
    }
    let archive =
        env::temp_dir().join(format!("lilexe-image-{}-{}.tar", std::process::id(), now()));
    let status = ProcessCommand::new(&docker)
        .args(["save", "-o"])
        .arg(&archive)
        .arg(&tag)
        .status()?;
    if !status.success() {
        bail!("docker save failed for '{tag}'");
    }
    let loaded = Image::load(&archive, vec![tag.clone()]).await;
    let _ = fs::remove_file(&archive);
    loaded?;
    Ok(tag)
}

pub(crate) async fn teardown(app: &App, row: &BoxRow) -> Result<()> {
    if let (Some(ts), Some(port)) = (&app.tailscale, row.serve_port) {
        let verb = if row.public { "funnel" } else { "serve" };
        let _ = run_external(ts, &[verb, &format!("--https={port}"), "off"]);
    }
    stop_and_remove(&row.name).await?;
    app.db
        .execute("DELETE FROM boxes WHERE name=?1", [&row.name])?;
    Ok(())
}
