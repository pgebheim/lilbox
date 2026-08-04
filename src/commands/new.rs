use std::env;

use anyhow::{Context, Result, bail};
use chrono::Local;
use rusqlite::params;

use crate::app::App;
use crate::cli::NewArgs;
use crate::provision::{build_template_image, provision};
use crate::sandbox::{SandboxSettings, configure_builder, with_secret_env};
use crate::tailscale::{
    CONTROL_PLANE_HOST, DEFAULT_AUTH_KEY_ENV, is_valid_env_var_name, node_hostname,
    require_auth_key, resolve_tag, tailscale_up_command, validate_tag,
};
use crate::util::{
    DEFAULT_GUEST_PORT, DEFAULT_IMAGE, alloc_host_port, now, parse_duration, parse_memory,
    random_name,
};

pub(crate) async fn cmd_new(app: &App, args: NewArgs) -> Result<()> {
    let name = match args.name {
        Some(name) => name,
        None => random_name(app)?,
    };
    if app.row(&name)?.is_some() {
        bail!("box '{name}' already exists");
    }
    let config = app.config()?;
    let tag = resolve_tag(args.tag.as_deref(), config.tailscale.tag.as_deref());
    let key_env = config
        .tailscale
        .auth_key_env
        .clone()
        .unwrap_or_else(|| DEFAULT_AUTH_KEY_ENV.into());
    let joins_tailnet = if !is_valid_env_var_name(&key_env) {
        eprintln!(
            "warning: tailscale authKeyEnv '{key_env}' is not a valid environment variable name; skipping tailnet join"
        );
        false
    } else {
        require_auth_key(env::var(&key_env).ok(), &key_env).is_ok()
    };
    // Only validate the tag when it will actually be used to join: an explicitly
    // passed --tag is still validated for fast feedback, but a malformed tag left
    // over in config shouldn't abort `new` when there's no auth key to join with.
    if joins_tailnet || args.tag.is_some() {
        validate_tag(&tag)?;
    }
    let template = args
        .template
        .as_deref()
        .map(|n| app.template(n))
        .transpose()?;
    let image = if let Some(image) = args.image {
        image
    } else if let Some(image) = template.as_ref().and_then(|t| t.manifest.image.clone()) {
        image
    } else if let Some(t) = template.as_ref().filter(|t| t.dockerfile) {
        build_template_image(t, args.rebuild).await?
    } else {
        config.image.clone().unwrap_or_else(|| DEFAULT_IMAGE.into())
    };
    let guest_port = args
        .port
        .or_else(|| template.as_ref().and_then(|t| t.manifest.port))
        .or(config.port)
        .unwrap_or(DEFAULT_GUEST_PORT);
    let mut cpus = args
        .cpus
        .or_else(|| template.as_ref().and_then(|t| t.manifest.cpus))
        .or(config.cpus);
    let mut memory_text = args
        .memory
        .or_else(|| template.as_ref().and_then(|t| t.manifest.memory.clone()))
        .or(config.memory);
    if let (Some(value), Some(max)) = (cpus, config.max_cpus)
        && value > max
    {
        eprintln!("warning: cpus {value} exceeds max_cpus {max}; clamping");
        cpus = Some(max);
    }
    if let (Some(value), Some(max)) = (&memory_text, &config.max_memory)
        && parse_memory(value)? > parse_memory(max)?
    {
        eprintln!("warning: memory {value} exceeds max_memory {max}; clamping");
        memory_text = Some(max.clone());
    }
    let memory = memory_text.as_deref().map(parse_memory).transpose()?;
    let ttl = args.ttl.or(config.ttl);
    let expires = ttl
        .as_deref()
        .map(parse_duration)
        .transpose()?
        .map(|s| now() + s as i64);
    let idle = args
        .idle_timeout
        .or(config.idle_timeout)
        .as_deref()
        .map(parse_duration)
        .transpose()?;
    let volume = if args.no_persist {
        None
    } else {
        Some(args.volume.unwrap_or_else(|| format!("lilbox-{name}-home")))
    };
    let host_port = alloc_host_port()?;
    println!(
        "booting {image} microVM as {name}{} ...",
        if volume.is_some() {
            " (persistent)"
        } else {
            ""
        }
    );
    let mut builder = configure_builder(SandboxSettings {
        name: &name,
        image: &image,
        host_port,
        guest_port,
        cpus,
        memory,
        idle_timeout: idle,
        volume: volume.as_deref(),
    });
    if joins_tailnet {
        builder = with_secret_env(builder, &key_env, CONTROL_PLANE_HOST);
    }
    let sandbox = builder
        .create_detached()
        .await
        .with_context(|| "microsandbox create failed")?;
    app.db.execute(
        "INSERT INTO boxes(name,image,guest_port,host_port,created,template,volume,expires) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![name, image, guest_port, host_port, Local::now().format("%Y-%m-%d %H:%M:%S").to_string(), template.as_ref().map(|t| &t.name), volume, expires],
    )?;
    println!("box {name} is up ({image}, guest :{guest_port})");
    if joins_tailnet {
        let argv = tailscale_up_command(&tag, &key_env, &name);
        match sandbox.exec(argv[0].clone(), argv[1..].to_vec()).await {
            Ok(output) if output.status().success => {
                if let Some(node) = output.stdout().ok().as_deref().and_then(node_hostname)
                    && let Err(error) = app.db.execute(
                        "UPDATE boxes SET tailscale_node=?1 WHERE name=?2",
                        params![node, name],
                    )
                {
                    eprintln!("warning: could not record tailscale node for '{name}': {error}");
                }
            }
            Ok(output) => eprintln!(
                "warning: could not join tailnet for '{name}': {}",
                output.stderr().unwrap_or_default().trim()
            ),
            Err(error) => {
                eprintln!("warning: could not join tailnet for '{name}': {error}")
            }
        }
    }
    if let Some(template) = &template {
        provision(app, &name, template).await?;
    }
    println!("  lilbox exec {name} -- <cmd>\n  lilbox ssh {name}\n  lilbox expose {name}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use microsandbox::Sandbox;

    use super::*;

    #[tokio::test]
    async fn tailscale_auth_key_is_stored_as_an_environment_reference() {
        let config = with_secret_env(
            Sandbox::builder("tailscale-secret-test").image("python"),
            "TS_AUTHKEY",
            CONTROL_PLANE_HOST,
        )
        .build()
        .await
        .unwrap();
        let serialized = serde_json::to_string(&config).unwrap();

        // The actual key value is resolved by the microsandbox runtime at sandbox
        // start (via env lookup), so it never reaches the serialized config by
        // construction. The meaningful guarantee is the env-reference below.
        assert!(serialized.contains("TS_AUTHKEY"));
        assert!(serialized.contains("\"kind\":\"env\""));
    }
}
