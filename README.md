# lilbox

**A lil [exe.dev](https://exe.dev) you run yourself.** Instant, isolated Linux
boxes on your own machine, publishable to your tailnet over HTTPS with one
command.

exe.dev's pitch is *"VMs, on the internet, quickly"* — SSH into a fresh Linux
box, run untrusted/AI-generated code in a disposable sandbox, and publish HTTP
endpoints to the world. `lilbox` reproduces that experience on hardware **you**
control by gluing together two pieces you can self-host:

| Layer | exe.dev | lilbox |
|---|---|---|
| Isolation | proprietary VMs | [**microsandbox**](https://microsandbox.dev) — libkrun microVMs (real kernel per box, KVM-isolated) |
| Publishing | HTTP proxies + custom domains | [**Tailscale**](https://tailscale.com) Serve (tailnet HTTPS) / Funnel (public) |
| Runtime | service-managed VMs | native Rust + embedded `microsandbox` SDK |
| State | Go + SQLite ("GUTS") | Rust + SQLite |

Each box is a genuine microVM — it boots its own Linux kernel in ~1–2s, not a
container sharing yours.

## Why microVMs (not containers)

`lilbox exec` on a box reports a **different kernel version than the host** — because
it *is* a different kernel. libkrun gives each box a KVM-backed boundary, which
is the isolation you actually want when running code an LLM just wrote.

## Requirements

- Linux host with KVM (`lilbox doctor` checks it; nested virtualization is fine)
- Rust 1.85+ and Cargo (build-time only)
- [Tailscale](https://tailscale.com) (only needed for `lilbox expose`)

The Rust SDK downloads its matching runtime components on the first build/use.
The separate `msb` executable is not required.

## Install

```bash
git clone <this repo> && cd lilbox
./install.sh            # builds and installs lilbox into ~/.local/bin
lilbox doctor               # verify the runtime
```

For development, use `cargo run -- <command>`.

## Quickstart

```bash
lilbox new web                       # boot a persistent python microVM named "web"
lilbox exec web -- python3 -V        # run a command inside it
lilbox ssh web                       # drop into a shell

# start something listening on the box's port 8000, then:
lilbox expose web                    # -> https://<you>.<tailnet>.ts.net:8443/
lilbox expose web --public           # -> public URL via Tailscale Funnel

lilbox ls                            # see every box + its published URL
lilbox fork web staging              # snapshot + clone (state and all)
lilbox rm web                        # tear down (also unpublishes)
```

Disposable, exe.dev-style sandbox for a one-off:

```bash
lilbox run -- python3 -c 'print("ran in a throwaway microVM")'
```

## Commands

| Command | Does |
|---|---|
| `lilbox new [NAME] [--template T] [--image I] [--port P] [--cpus N] [--memory M] [--rebuild] [--no-persist] [--volume V] [--ttl D] [--idle-timeout D]` | Boot a persistent box (default image `python`, guest port `8000`, persistent `/root` home) |
| `lilbox templates` | List available box templates |
| `lilbox provision NAME` | Re-run a box's template setup script |
| `lilbox ls [--json]` | List boxes with live status + published URLs (`--json` for scripting) |
| `lilbox gc` | Reap boxes whose `--ttl` has elapsed (cron-friendly) |
| `lilbox exec NAME -- CMD…` | Run a command inside a box |
| `lilbox ssh NAME [-- CMD]` | Interactive shell (or one-shot command) |
| `lilbox cp SRC DST` | Copy files to/from a box (box side = `NAME:/path`) |
| `lilbox logs NAME [-f] [--tail N] [--source S]` | Show or follow captured output |
| `lilbox run [--image I] -- CMD…` | Ephemeral box: boot, run, discard |
| `lilbox rebuild NAME [--image X]` | Recreate a box on a new/updated image, keeping its home volume |
| `lilbox agent [NAME] [--workspace D\|--clone URL] [--agents-file F] -- task` | Run a coding agent (Claude Code) in a box against a mounted workspace |
| `lilbox expose NAME [--public]` | Publish the box over HTTPS (tailnet, or Funnel with `--public`) |
| `lilbox unexpose NAME` | Stop publishing |
| `lilbox url NAME` | Print the published URL |
| `lilbox stop/start/restart NAME` | Lifecycle |
| `lilbox fork NAME [NEWNAME]` | Snapshot a box and boot a clone from it |
| `lilbox volumes` | List persistent home volumes (and orphans) |
| `lilbox image load ARCHIVE --tag TAG` | Import an OCI/Docker archive through the Rust SDK |
| `lilbox image ls` | List the embedded runtime's image cache |
| `lilbox rm NAME [--keep-data]` | Remove a box + unpublish; deletes its home volume unless `--keep-data` |
| `lilbox stat NAME` | Detailed box info |
| `lilbox doctor` | Check the embedded microsandbox runtime, KVM, Tailscale, and tailnet |

## Templates

A **template** is a box that arrives useful — an image + defaults + an optional
post-boot setup script. It's a directory `<name>/`:

```
<name>/
  template.json      # image + defaults (cpus/memory/port)
  setup.sh           # optional: run inside the box after boot (idempotent)
  Dockerfile         # optional: build the image locally instead of pulling
```

```bash
lilbox templates                      # list templates (repo starters + your own)
lilbox new dev --template python-dev   # boot from a template
lilbox provision dev                   # re-run its setup (idempotent)
```

Resolution: user templates in `~/.lilbox/templates/` **override** repo starters
in `templates/`. Precedence for values is **CLI flag > template > default**
(`lilbox new --image alpine --template python-dev` boots alpine, keeping the
template's other defaults). If a template has a `Dockerfile` (and no `image`),
`lilbox new` builds it with Docker, imports it through `Image::load`, and caches it; `--rebuild`
forces a fresh build. `setup.sh` runs post-boot; a non-zero exit is surfaced and
logged to `~/.lilbox/logs/<box>-setup.log` (the box is kept for inspection).

Shipped starters: **`python-dev`** (python + git + uv) and **`node-dev`**
(node + npm + git).

## How publishing works

`lilbox new` maps a guest port to a random host loopback port using
`SandboxBuilder::port`. `lilbox expose` then points
`tailscale serve` at that host port on a dedicated HTTPS port (8443+), so every
box gets its own URL and nothing collides with an existing root `serve`/Caddy
setup. `--public` swaps `serve` for `funnel` (443/8443/10000 only) to reach the
open internet. State (name → image → host port → serve port → URL) lives in
`~/.lilbox/state.db`.

## Boxes with Tailscale baked in (`lilbox-box`)

The default images (`python`, `alpine`, …) are published from the *host* via
`tailscale serve`. The [`lilbox-box`](images/lilbox-box/) image instead bakes
`tailscale` + `tailscaled` and a first-boot bring-up hook *into the box*, so it
can join your tailnet as its own node — the foundation for per-box hostnames,
keyless `lilbox ssh`, and identity-based auth (epic #2).

```bash
images/lilbox-box/build.sh          # docker build -> lilbox image load
lilbox new mybox --image lilbox-box     # boot it
lilbox exec mybox -- tailscale version  # tailscale is baked in
```

See [`images/lilbox-box/README.md`](images/lilbox-box/README.md) for details.

## Persistent volumes (devboxes)

Every `lilbox new` box gets a named microsandbox volume `lilbox-<name>-home` mounted at
`/root`, so its home **survives `lilbox stop`/`start` and image swaps** — a sandbox
becomes a devbox. Pass `--no-persist` for a throwaway home (`lilbox run` is always
ephemeral).

```bash
lilbox new dev                     # persistent /root by default
lilbox rebuild dev --image python  # swap the image, keep the home data
lilbox rm dev --keep-data          # remove the box but preserve its volume
lilbox new dev                     # a same-named box re-attaches it
lilbox new dev2 --volume lilbox-dev-home   # or adopt it explicitly by name
lilbox volumes                     # list volumes (orphans flagged)
```

`lilbox rm` **deletes the home volume by default** (unless `--keep-data`, or another
box still uses it). `lilbox rebuild` never touches the volume — on a boot failure the
data is preserved and a bare `lilbox rebuild NAME` retries.

## Lifecycle & config (cheap when idle)

- **TTL:** `lilbox new web --ttl 2h` auto-expires the box; `lilbox gc` (run it from cron)
  reaps expired boxes. `lilbox run --ttl 30m` bounds an ephemeral box.
- **Idle suspend:** `lilbox new web --idle-timeout 20m` suspends the box after it's
  idle (frees RAM); `lilbox exec`/`lilbox ssh` **transparently resume** it (~1–2s). A box
  you `lilbox stop` yourself is never auto-resumed.
- **Defaults & caps:** `~/.lilbox/config.toml` sets defaults for
  `image`/`port`/`cpus`/`memory`/`ttl`/`idle_timeout` and caps
  (`max_cpus`/`max_memory`) that clamp an over-limit `lilbox new`. Precedence: CLI
  flag > template > config > built-in. See
  [`config.toml.example`](config.toml.example).

## Agent-in-a-box

`lilbox agent` runs a coding agent (Claude Code) **inside** a microVM against a
mounted workspace — safe by construction, since AI-generated code runs behind
the KVM boundary, not on your host.

```bash
lilbox agent -- "add a test for the parser"        # agent works in the current dir
lilbox agent --clone https://github.com/you/repo -- "fix the failing CI"
lilbox agent --agents-file ./AGENTS.md -- "..."     # pass repo agent instructions
```

The workspace is bind-mounted at `/workspace`, so the agent's edits appear on
the host **as it works** — retrieve them directly (or `lilbox expose` what it built).
Bring your own key: `export ANTHROPIC_API_KEY=…` and it's injected via
the SDK's `secret_env` API — sent only to `api.anthropic.com`
and **never written into the box's image or state**. Override with
`--key-env`/`--key-host`.

## Configuration

`~/.lilbox/config.toml` sets `lilbox new` defaults using standard TOML. Precedence
is **CLI flag > template > config > built-in default**. See
[`config.toml.example`](config.toml.example).

## Testing

Run the Rust unit tests and compile checks with Cargo:

```bash
cargo test --locked
cargo check --locked
```

The live end-to-end smoke test needs a KVM host and network access for image
pulls.

## What this POC does *not* do (yet)

- **No auth on published endpoints** beyond what your tailnet already enforces.
  `--public` is genuinely public — anything the box serves is on the internet.
- **One published web port per box.** Multi-port / arbitrary later ports would
  mean re-creating the box (microsandbox fixes published ports at boot).
- **No custom domains / per-box DNS names.** Boxes are distinguished by HTTPS
  port, not hostname (a Caddy vhost layer could add this).
- **Not multi-tenant.** Single user, single host.

## License

MIT
