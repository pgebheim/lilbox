# `lilexe-box` — a base box image with Tailscale baked in

The foundation for **per-box tailnet identity** (epic #2): a box that already
ships `tailscale` + `tailscaled` and a first-boot bring-up hook, so it can join
your tailnet as its *own* node instead of being published from the host.

This directory builds that image. Joining the tailnet as a tagged, ephemeral
node (`tailscale up --auth-key=… --advertise-tags=tag:lilexe-box --ssh`) is
layered on by `vm new` in a later step (#4); this image bakes in only the
binaries and the daemon bring-up.

## What's in it

| File | Role |
|---|---|
| `Dockerfile` | `alpine:3.20` + `tailscale` (ships `tailscale` **and** `tailscaled`), `iproute2`, `ca-certificates`, `openssh`. |
| `lilexe-boot` | First-boot hook: starts `tailscaled` on a **real kernel tun** (`/dev/net/tun` + `CAP_NET_ADMIN` are present in the libkrun guest), falling back to userspace networking only if tun is absent. Idempotent. |
| `build.sh` | `docker build` -> `docker save` -> native `vm image load`. |

Tailscale version tracks the pinned Alpine release (`3.20` → tailscale `1.66.4`).
Bump the `FROM` line to move it.

## Build & load

microsandbox has no native image build; it consumes OCI images. Build with
Docker and import the result through the Rust SDK:

```bash
images/lilexe-box/build.sh          # docker build + vm image load
```

Requires `docker` (to build) and `vm` (to import). No registry is needed; the
image is loaded straight into microsandbox's cache under the tag `lilexe-box`.

## Boot it

```bash
vm new mybox --image lilexe-box     # boots the box
vm exec mybox -- tailscale version  # -> 1.66.4  (tailscale is baked in)
```

The `lilexe-boot` hook starts `tailscaled` (bringing up a `tailscale0` kernel
tun); a later step runs `tailscale up` with an injected ephemeral, tagged auth
key so the box appears in `tailscale status` as its own `tag:lilexe-box` node.
