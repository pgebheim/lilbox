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
| `vm new [NAME] [--template T] [--image I] [--port P] [--cpus N] [--memory M] [--rebuild]` | Boot a persistent box (default image `python`, publishes guest port `8000`) |
| `vm templates` | List available box templates |
| `vm provision NAME` | Re-run a box's template setup script |
| `vm ls` | List boxes with live status + published URLs |
| `vm exec NAME -- CMD…` | Run a command inside a box |
| `vm ssh NAME [-- CMD]` | Interactive shell (or one-shot command) |
| `vm run [--image I] -- CMD…` | Ephemeral box: boot, run, discard |
| `vm expose NAME [--public]` | Publish the box over HTTPS (tailnet, or Funnel with `--public`) |
| `vm unexpose NAME` | Stop publishing |
| `vm url NAME` | Print the published URL |
| `vm stop/start/restart NAME` | Lifecycle |
| `vm fork NAME [NEWNAME]` | Snapshot a box and boot a clone from it |
| `vm rm NAME` | Remove a box (and unpublish it) |
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

## What this POC does *not* do (yet)

- **No auth on published endpoints** beyond what your tailnet already enforces.
  `--public` is genuinely public — anything the box serves is on the internet.
- **One published web port per box.** Multi-port / arbitrary later ports would
  mean re-creating the box (microsandbox fixes published ports at boot).
- **No custom domains / per-box DNS names.** Boxes are distinguished by HTTPS
  port, not hostname (a Caddy vhost layer could add this).
- **No built-in agent** (exe.dev's "Shelley"). Drop your own agent in with
  `vm ssh` / `vm exec`.
- **Not multi-tenant.** Single user, single host.

## License

MIT
