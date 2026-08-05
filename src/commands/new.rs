use std::env;

use anyhow::{Context, Result, bail};
use chrono::Local;
use rusqlite::params;

use crate::app::App;
use crate::cli::NewArgs;
use crate::overlay;
use crate::provision::{build_template_image, provision};
use crate::sandbox::{SandboxSettings, configure_builder};
use crate::tailscale::{
    DEFAULT_AUTH_KEY_ENV, JoinMode, is_valid_env_var_name, join_failure_detail, mint_ephemeral_key,
    node_hostname, require_auth_key, resolve_join_mode, resolve_tag, tailscale_up_command,
    validate_tag, wants_tailnet,
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
    let mut tag = resolve_tag(args.tailnet_tag.as_deref(), config.tailscale.tag.as_deref());
    // Tailnet join is opt-in: a key alone in the environment no longer
    // triggers anything. It fires only on explicit intent (--tailnet,
    // --tailnet-tag, or [tailscale] auto = true) plus a resolvable credential.
    let want_tailnet = wants_tailnet(
        args.tailnet,
        args.tailnet_tag.as_deref(),
        config.tailscale.auto,
    );
    let mut key_env = DEFAULT_AUTH_KEY_ENV.to_string();
    let mut minted_key: Option<String> = None;
    let mut joins_tailnet = false;
    if want_tailnet {
        let mode = resolve_join_mode(&config.tailscale, args.tailnet_tag.as_deref(), |name| {
            env::var(name).ok()
        });
        joins_tailnet = match mode {
            JoinMode::Mint {
                tag: mint_tag,
                client_id,
                client_secret,
            } => {
                tag = mint_tag;
                // A malformed tag from config must not abort `new` (never fail on a
                // tailnet problem) — warn and skip the join. An explicit --tailnet-tag
                // is still hard-validated below for fast feedback.
                if let Err(error) = validate_tag(&tag) {
                    eprintln!("warning: {error}; skipping tailnet join");
                    false
                } else {
                    match mint_ephemeral_key(&client_id, &client_secret, &tag, &name).await {
                        Ok(key) => {
                            minted_key = Some(key);
                            true
                        }
                        Err(error) => {
                            eprintln!(
                                "warning: could not mint tailnet auth key: {error}; skipping tailnet join"
                            );
                            false
                        }
                    }
                }
            }
            JoinMode::StaticEnv { key_env: env_name } => {
                key_env = env_name;
                if !is_valid_env_var_name(&key_env) {
                    eprintln!(
                        "warning: tailscale authKeyEnv '{key_env}' is not a valid environment variable name; skipping tailnet join"
                    );
                    false
                } else {
                    require_auth_key(env::var(&key_env).ok(), &key_env).is_ok()
                }
            }
            JoinMode::Skip => {
                eprintln!(
                    "note: --tailnet requested but no auth key set (TS_AUTHKEY / [tailscale] oauthClientId) — booting '{name}' without joining the tailnet"
                );
                false
            }
        };
    }
    // Only validate the tag when it will actually be used to join: an explicitly
    // passed --tailnet-tag is still validated for fast feedback, but a malformed
    // tag left over in config shouldn't abort `new` when there's no auth key to
    // join with.
    if joins_tailnet || args.tailnet_tag.is_some() {
        validate_tag(&tag)?;
    }
    let template = args
        .template
        .as_deref()
        .map(|n| app.template(n))
        .transpose()?;
    let mut image = if let Some(image) = args.image {
        image
    } else if let Some(image) = template.as_ref().and_then(|t| t.manifest.image.clone()) {
        image
    } else if let Some(t) = template.as_ref().filter(|t| t.dockerfile) {
        build_template_image(t, args.rebuild).await?
    } else {
        config.image.clone().unwrap_or_else(|| DEFAULT_IMAGE.into())
    };
    let is_tailnet_capable = image == "lilbox-box" || image.starts_with("lilbox/tailnet/");
    if joins_tailnet && !is_tailnet_capable {
        match overlay::ensure_tailnet_image(&image, args.rebuild).await {
            Ok(t) => image = t,
            Err(e) => eprintln!(
                "warning: could not build a tailnet-capable image for '{image}': {e:#}; booting the base as-is"
            ),
        }
    } else if !want_tailnet && is_tailnet_capable {
        eprintln!("note: '{image}' is tailnet-capable; pass --tailnet to join the tailnet");
    }
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
    let builder = configure_builder(SandboxSettings {
        name: &name,
        image: &image,
        host_port,
        guest_port,
        cpus,
        memory,
        idle_timeout: idle,
        volume: volume.as_deref(),
    });
    let sandbox = builder.create_detached().await.with_context(|| {
        format!(
            "could not create box '{name}' from image '{image}' \
             (if '{image}' is a locally-built image, load it first — \
             e.g. images/<name>/build.sh — and check `lilbox image ls`)"
        )
    })?;
    app.db.execute(
        "INSERT INTO boxes(name,image,guest_port,host_port,created,template,volume,expires) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![name, image, guest_port, host_port, Local::now().format("%Y-%m-%d %H:%M:%S").to_string(), template.as_ref().map(|t| &t.name), volume, expires],
    )?;
    println!("box {name} is up ({image}, guest :{guest_port})");
    if joins_tailnet {
        // Tailscale requires the guest to present its own auth key to
        // `tailscale up` -- there is no host-side "join for me" call, so the
        // box necessarily sees the real key. Unlike the agent's Anthropic key
        // (plain HTTPS, so the builder's placeholder-secret + `allow_host` TLS
        // substitution works), `tailscaled` talks to controlplane.tailscale.com
        // over the encrypted noise protocol, which the substitution can't
        // intercept -- so the real value must reach the guest directly. We
        // deliver it as a transient env var on this one exec (never through
        // the builder, so it's never written into the sandbox's persisted
        // config/state). Prefer ephemeral, single-use keys (the OAuth mint
        // path above already does this) so the guest's momentary visibility
        // of its own key is moot.
        let auth_key = minted_key.take().or_else(|| env::var(&key_env).ok());
        match auth_key {
            Some(auth_key) => {
                let argv = tailscale_up_command(&tag, &key_env, &name, guest_port);
                let result = sandbox
                    .exec_with(argv[0].clone(), |e| {
                        e.args(argv[1..].to_vec())
                            .env(key_env.as_str(), auth_key.as_str())
                    })
                    .await;
                drop(auth_key);
                match result {
                    Ok(output) if output.status().success => {
                        if let Some(node) = output.stdout().ok().as_deref().and_then(node_hostname)
                            && let Err(error) = app.db.execute(
                                "UPDATE boxes SET tailscale_node=?1 WHERE name=?2",
                                params![node, name],
                            )
                        {
                            eprintln!(
                                "warning: could not record tailscale node for '{name}': {error}"
                            );
                        }
                    }
                    Ok(output) => eprintln!(
                        "warning: could not join tailnet for '{name}': {}",
                        join_failure_detail(
                            output.status().code,
                            &output.stderr().unwrap_or_default(),
                            &image,
                        )
                    ),
                    Err(error) => {
                        eprintln!("warning: could not join tailnet for '{name}': {error}")
                    }
                }
            }
            None => {
                eprintln!(
                    "warning: could not resolve tailnet auth key for '{name}'; skipping tailnet join"
                );
            }
        }
    }
    if let Some(template) = &template {
        provision(app, &name, template).await?;
    }
    println!("  lilbox exec {name} -- <cmd>\n  lilbox ssh {name}\n  lilbox expose {name}");
    Ok(())
}
