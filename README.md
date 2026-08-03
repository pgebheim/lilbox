# lilexe

**A lil [exe.dev](https://exe.dev) you run yourself.** Instant, isolated Linux
boxes on your own machine, publishable to your tailnet over HTTPS with one
command.

exe.dev's pitch is *"VMs, on the internet, quickly"* — SSH into a fresh Linux
box, run untrusted/AI-generated code in a disposable sandbox, and publish HTTP
endpoints to the world. `lilexe` reproduces that experience on hardware **you**
control by gluing together two pieces you can self-host:

| Layer | exe.dev | lilexe |
|---|---|---|
| Isolation | proprietary VMs | [**microsandbox**](https://microsandbox.dev) — libkrun microVMs (real kernel per box, KVM-isolated) |
| Publishing | HTTP proxies + custom domains | [**Tailscale**](https://tailscale.com) Serve (tailnet HTTPS) / Funnel (public) |
| State | Go + SQLite ("GUTS") | one Python file + SQLite |

Each box is a genuine microVM — it boots its own Linux kernel in ~1–2s, not a
container sharing yours.

## Why microVMs (not containers)

`vm exec` on a box reports a **different kernel version than the host** — because
it *is* a different kernel. libkrun gives each box a KVM-backed boundary, which
is the isolation you actually want when running code an LLM just wrote.

## Requirements

- Linux host with KVM (`msb doctor` must pass — nested virt is fine)
- [microsandbox](https://microsandbox.dev): `curl -fsSL https://install.microsandbox.dev | sh`
- [Tailscale](https://tailscale.com) (only needed for `vm expose`)
- Python 3.8+ (stdlib only — no pip installs)

## Install

```bash
git clone <this repo> && cd lilexe
./install.sh            # symlinks bin/vm into ~/.local/bin
vm doctor               # verify the runtime
```

Or just run `./bin/vm` in place.

## Quickstart

```bash
vm new web                       # boot a persistent python microVM named "web"
vm exec web -- python3 -V        # run a command inside it
vm ssh web                       # drop into a shell

# start something listening on the box's port 8000, then:
vm expose web                    # -> https://<you>.<tailnet>.ts.net:8443/
vm expose web --public           # -> public URL via Tailscale Funnel

vm ls                            # see every box + its published URL
vm fork web staging              # snapshot + clone (state and all)
vm rm web                        # tear down (also unpublishes)
```

Disposable, exe.dev-style sandbox for a one-off:

```bash
vm run -- python3 -c 'print("ran in a throwaway microVM")'
```

## Commands

| Command | Does |
|---|---|
| `vm new [NAME] [--template T] [--image I] [--port P] [--cpus N] [--memory M] [--rebuild] [--no-persist] [--volume V] [--ttl D] [--idle-timeout D]` | Boot a persistent box (default image `python`, guest port `8000`, persistent `/root` home) |
| `vm templates` | List available box templates |
| `vm provision NAME` | Re-run a box's template setup script |
| `vm ls` | List boxes with live status + published URLs |
| `vm gc` | Reap boxes whose `--ttl` has elapsed (cron-friendly) |
| `vm exec NAME -- CMD…` | Run a command inside a box |
| `vm ssh NAME [-- CMD]` | Interactive shell (or one-shot command) |
| `vm run [--image I] -- CMD…` | Ephemeral box: boot, run, discard |
| `vm rebuild NAME [--image X]` | Recreate a box on a new/updated image, keeping its home volume |
| `vm agent [NAME] [--workspace D\|--clone URL] [--agents-file F] -- task` | Run a coding agent (Claude Code) in a box against a mounted workspace |
| `vm expose NAME [--public]` | Publish the box over HTTPS (tailnet, or Funnel with `--public`) |
| `vm unexpose NAME` | Stop publishing |
| `vm url NAME` | Print the published URL |
| `vm stop/start/restart NAME` | Lifecycle |
| `vm fork NAME [NEWNAME]` | Snapshot a box and boot a clone from it |
| `vm volumes` | List persistent home volumes (and orphans) |
| `vm rm NAME [--keep-data]` | Remove a box + unpublish; deletes its home volume unless `--keep-data` |
| `vm stat NAME` | Detailed box info |
| `vm doctor` | Check msb + tailscale + tailnet |

## Templates

A **template** is a box that arrives useful — an image + defaults + an optional
post-boot setup script. It's a directory `<name>/`:

```
<name>/
  template.json      # image + defaults (cpus/memory/port) — JSON, stdlib-only
  setup.sh           # optional: run inside the box after boot (idempotent)
  Dockerfile         # optional: build the image locally instead of pulling
```

```bash
vm templates                      # list templates (repo starters + your own)
vm new dev --template python-dev   # boot from a template
vm provision dev                   # re-run its setup (idempotent)
```

Resolution: user templates in `~/.lilexe/templates/` **override** repo starters
in `templates/`. Precedence for values is **CLI flag > template > default**
(`vm new --image alpine --template python-dev` boots alpine, keeping the
template's other defaults). If a template has a `Dockerfile` (and no `image`),
`vm new` builds it (`docker build` → `msb load`) and caches it; `--rebuild`
forces a fresh build. `setup.sh` runs post-boot; a non-zero exit is surfaced and
logged to `~/.lilexe/logs/<box>-setup.log` (the box is kept for inspection).

Shipped starters: **`python-dev`** (python + git + uv) and **`node-dev`**
(node + npm + git).

## How publishing works

`vm new` maps a guest port to a random host loopback port
(`msb create --port <host>:<guest>`). `vm expose` then points
`tailscale serve` at that host port on a dedicated HTTPS port (8443+), so every
box gets its own URL and nothing collides with an existing root `serve`/Caddy
setup. `--public` swaps `serve` for `funnel` (443/8443/10000 only) to reach the
open internet. State (name → image → host port → serve port → URL) lives in
`~/.lilexe/state.db`.

## Boxes with Tailscale baked in (`lilexe-box`)

The default images (`python`, `alpine`, …) are published from the *host* via
`tailscale serve`. The [`lilexe-box`](images/lilexe-box/) image instead bakes
`tailscale` + `tailscaled` and a first-boot bring-up hook *into the box*, so it
can join your tailnet as its own node — the foundation for per-box hostnames,
keyless `vm ssh`, and identity-based auth (epic #2).

```bash
images/lilexe-box/build.sh          # docker build -> msb load as `lilexe-box`
vm new mybox --image lilexe-box     # boot it
vm exec mybox -- tailscale version  # tailscale is baked in
```

See [`images/lilexe-box/README.md`](images/lilexe-box/README.md) for details.

## Persistent volumes (devboxes)

Every `vm new` box gets a named msb volume `lilexe-<name>-home` mounted at
`/root`, so its home **survives `vm stop`/`start` and image swaps** — a sandbox
becomes a devbox. Pass `--no-persist` for a throwaway home (`vm run` is always
ephemeral).

```bash
vm new dev                     # persistent /root by default
vm rebuild dev --image python  # swap the image, keep the home data
vm rm dev --keep-data          # remove the box but preserve its volume
vm new dev                     # a same-named box re-attaches it
vm new dev2 --volume lilexe-dev-home   # or adopt it explicitly by name
vm volumes                     # list volumes (orphans flagged)
```

`vm rm` **deletes the home volume by default** (unless `--keep-data`, or another
box still uses it). `vm rebuild` never touches the volume — on a boot failure the
data is preserved and a bare `vm rebuild NAME` retries.

## Lifecycle & config (cheap when idle)

- **TTL:** `vm new web --ttl 2h` auto-expires the box; `vm gc` (run it from cron)
  reaps expired boxes. `vm run --ttl 30m` bounds an ephemeral box.
- **Idle suspend:** `vm new web --idle-timeout 20m` suspends the box after it's
  idle (frees RAM); `vm exec`/`vm ssh` **transparently resume** it (~1–2s). A box
  you `vm stop` yourself is never auto-resumed.
- **Defaults & caps:** `~/.lilexe/config.toml` sets defaults for
  `cpus`/`memory`/`ttl`/`idle_timeout` and caps (`max_cpus`/`max_memory`) that
  clamp an over-limit `vm new`. Precedence: CLI flag > config > built-in. See
  [`config.toml.example`](config.toml.example).

## Agent-in-a-box

`vm agent` runs a coding agent (Claude Code) **inside** a microVM against a
mounted workspace — safe by construction, since AI-generated code runs behind
the KVM boundary, not on your host.

```bash
vm agent -- "add a test for the parser"        # agent works in the current dir
vm agent --clone https://github.com/you/repo -- "fix the failing CI"
vm agent --agents-file ./AGENTS.md -- "..."     # pass repo agent instructions
```

The workspace is bind-mounted at `/workspace`, so the agent's edits appear on
the host **as it works** — retrieve them directly (or `vm expose` what it built).
Bring your own key: `export ANTHROPIC_API_KEY=…` and it's injected via
microsandbox secret injection (`--secret`) — sent only to `api.anthropic.com`
and **never written into the box's image or state**. Override with
`--key-env`/`--key-host`.

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
