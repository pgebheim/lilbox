# lilbox

## All development happens in a worktree

The primary checkout stays on `main`. Never branch, edit, or commit there —
every change gets its own worktree under `.claude/worktrees/`, however small.
That includes one-off fixes, doc edits, config tweaks, and spikes, not just
features.

Create one with the shared bootstrap (it fetches first, branches from the right
base, and symlinks env files):

```bash
.claude/scripts/setup-worktree.sh <branch> --base origin/main --no-install
```

- **`--no-install` is required here.** `runtime.packageManager` is `none` — this
  is a Cargo repo and the script's default install command is `npm install`.
- Branch names follow `vcs.branchConvention` in `.rig/config.json`:
  `{user}/{ticket}-{slug}`.
- The worktree lands at `.claude/worktrees/<last segment of branch>` by default;
  the absolute path is the last line of stdout, so
  `WT=$(.claude/scripts/setup-worktree.sh … | tail -1)`.

Tear it down once its PR merges:

```bash
.claude/scripts/remove-worktree.sh <branch>
```

Teardown skips a dirty worktree rather than failing, so it's safe to run over
all of them. `git worktree list` shows what's currently checked out.

**Commit early.** A worktree's uncommitted edits are lost if anything reaps it,
and reaping is routine here — get work onto its branch rather than leaving it
resident in the working tree.

`/rig-worktree` and the implement-style flows (`/rig-task`, `/rig-epic`,
`/rig-sprint`) already set this up for you. The rule is here for everything
that doesn't go through them.

## Build and test

```bash
cargo test --locked
cargo check --locked
cargo fmt --check
```

CI additionally runs `cargo clippy --locked --all-targets -- -D warnings`, so
run that before pushing.

The live end-to-end paths need a KVM host and network access for image pulls;
`lilbox doctor` must pass on such a host before those can be exercised.

## Repository layout

- `src/` — the `lilbox` binary crate (single binary, `src/main.rs`).
  - `src/commands/` — one module per command group (`new`, `agent`,
    `lifecycle`, `net`, `template`, `view`, `cp`).
  - `src/overlay.rs` — Docker-free OCI image overlay builder (pulls a base
    image, appends a layer, emits an OCI layout tar).
  - `src/tailscale.rs` — tailnet join (ephemeral OAuth-minted keys), serve /
    funnel port management.
  - `src/app.rs` — XDG state/config/data dirs, SQLite state DB, legacy
    `~/.lilbox` migration.
- `templates/` — built-in box templates (Dockerfile + `setup.sh` +
  `template.json` per dir).
- `images/lilbox-box/` — the tailnet-capable base image build.
- `contrib/herdr/` — herdr plugin exposing lilbox as an agent sandbox backend.
- `tests/` — CLI integration tests (pinned to a temp HOME; no live KVM).

## Conventions

- Rust edition 2024, stable toolchain; keep `cargo fmt --check` and clippy
  (`-D warnings`) clean.
- User-facing state lives in XDG dirs (`~/.config/lilbox`,
  `~/.local/state/lilbox`, `~/.local/share/lilbox`) — never the legacy
  `~/.lilbox` except in the one-time migration path.
- Best-effort cleanup paths (tailnet logout, serve teardown, provisioning
  fallout) warn to stderr and must never block the primary operation.
- Secrets reach guests only via microsandbox's secret mechanism or transient
  exec env vars — never through the sandbox builder's persisted config.


<!-- rig:start -->
## Rig

This project uses [Rig](https://github.com/agent-rig/rig) skills — self-contained
markdown procedures. **When a request matches a skill's triggers, read that
file and follow it.** Project config lives in `.rig/config.json`.

- **rig-debug** — Systematic root-cause debugging for a failing test, production bug, flaky behavior, or unexplained error. Spawns the debugger agent through Phase 1 → 4 (root-cause → pattern analysis → hypothesis → minimal fix). Refuses to propose fixes before evidence is gathered. Triggers on: 'debug', 'why is X failing', 'root cause', 'this isn't working'.
  → read `.rig/skills/rig-debug.md` and follow it.
- **rig-doctor** — Preflight health check for a rig project: detect things that make rig degrade or fail — missing gh auth or the project scope, an unreachable board, an invalid config, no CI gates, missing review catalog — and report each with the exact fix. Diagnose-only by default; `--fix` applies the safe fixes with confirmation. Triggers on: 'doctor', 'rig doctor', 'check my setup', 'health check', 'why isn't rig working', 'diagnose rig', 'preflight'.
  → read `.rig/skills/rig-doctor.md` and follow it.
- **rig-epic** — Plan and run a multi-ticket epic: decompose a feature into parent + child items and stack PRs on a shared integration branch instead of landing each on main. Use when children interleave (one item's runtime contract depends on another's incomplete state) — stacking keeps each child PR reviewable without temporarily breaking main, then squashes to main once. Triggers on: 'epic', 'plan epic', 'plan this as an epic', 'break this into an epic', 'start epic', 'integration branch', 'stack PRs', 'finish epic'.
  → read `.rig/skills/rig-epic.md` and follow it.
- **rig-issue** — Create, view, move, or manage tickets in the project's issue tracker (Linear, GitHub Issues, or none). Triggers on: 'ticket', 'create ticket', 'move ticket', 'ticket board', 'show tickets', 'list tickets'.
  → read `.rig/skills/rig-issue.md` and follow it.
- **rig-plan** — Turn a written spec or PRD into a reviewed backlog. Reads a spec file, decomposes it into work — deciding per chunk whether it's an epic (interleaved, shared integration branch), a sprint (independent tickets), or a single ticket — shows the plan for approval, then materializes the tickets on the tracker + board with dependencies and shape labels. Does NOT start work: execution is handed to rig-epic / rig-task / rig-sprint. Triggers on: 'plan', 'rig-plan', 'break down the spec', 'turn this spec into tickets', 'backlog from spec', 'plan the work', 'decompose the PRD', 'plan from SPEC.md'.
  → read `.rig/skills/rig-plan.md` and follow it.
- **rig-review** — Local code review, both halves. `find` (default): walk the REVIEWER.md catalog + scope invariants against a diff, two adversarial passes, return triaged P0–P3 findings — read-only. `fix`: drive review feedback to convergence, from a PR review bot OR local `find` results (spawn coder → recheck until clean). Triggers on: 'review my changes', 'review the diff', 'pre-PR review', 'review gate', 'check the diff before pushing', 'review fix', 'address the review', 'apply review feedback', 'fix the bot', 'address codex review', 'fix the findings', 'review loop'.
  → read `.rig/skills/rig-review.md` and follow it.
- **rig-spike** — Run a time-boxed research spike to answer an open technical question or de-risk an approach BEFORE committing to implementation. Produces written findings + a recommendation (and optionally a throwaway prototype), not production code. Use when the team needs to evaluate feasibility, compare options, or reduce uncertainty before a feature is planned. Triggers on: 'spike', 'research spike', 'investigate', 'evaluate feasibility', 'de-risk', 'proof of concept', 'POC', 'compare options', 'can we', 'what would it take'.
  → read `.rig/skills/rig-spike.md` and follow it.
- **rig-sprint** — Plan or run a sprint of independent tickets (or ad-hoc tasks). Use 'plan <feature>' to decompose a feature into independent tickets (no integration branch — each lands on the trunk on its own). Pass ticket IDs (or task descriptions) to execute them in phased dependency order with cleanup between phases. With no args (or just 'plan'), previews the current sprint-ready queue without launching anything. Triggers on: 'sprint', 'plan sprint', 'plan this as a sprint', 'break this into tickets', 'run tickets', 'execute tickets', 'run these tickets', 'kick off tickets', 'start sprint'.
  → read `.rig/skills/rig-sprint.md` and follow it.
- **rig-sync** — Keep a repo in sync with its spec — the terraform loop for code. Treats the spec as desired state and the code as actual state, computes the drift between them (both directions), and reconciles it. `plan` (default): read-only drift report — what the spec demands that the code lacks, and what the code has that the spec doesn't. `apply`: reconcile the actionable drift through a pluggable sink — a durable Smithers workflow (default), a tracked milestone of tickets, or report-only — and it never edits product code directly. Triggers on: 'rig-sync', 'sync the repo to the spec', 'spec sync', 'spec drift', 'drift between spec and code', 'reconcile spec and code', 'is the code still in sync with the spec', 'what changed vs the spec', 'terraform for code', 'plan the spec drift'.
  → read `.rig/skills/rig-sync.md` and follow it.
- **rig-task** — Implement one unit of work end-to-end — from a tracker issue OR an ad-hoc description: spec review, TDD (RED→GREEN→REFACTOR), pre-PR self-review, open a PR, then drive the review-bot loop to clean. Runs start→finish in one shot by default; `start`/`finish` are optional phases for pause/resume. Sibling to /rig-epic (one unit vs many). Never auto-merges unless `--auto-merge` is passed (then CI is the merge gate). Triggers on: 'implement', 'implement this', 'work on', 'pick up', 'start task', 'start <ISSUE>', 'finish task'.
  → read `.rig/skills/rig-task.md` and follow it.
- **rig-tidy** — Run post-merge code cleanup: audit recent changes for dead code, duplicates, and stale comments, then safely remove them. Triggers on: 'cleanup', 'simplify', 'clean up code', 'remove dead code', 'post-merge cleanup'.
  → read `.rig/skills/rig-tidy.md` and follow it.
- **rig-worktree** — Manage isolated git worktrees: create one wired up for dev (fetch, branch from base, symlink env, install deps), list them with PR state, or safely remove merged ones. A shared bootstrap that implement-style flows can call, also usable directly. Triggers on: 'worktree', 'set up a worktree', 'new worktree', 'list worktrees', 'remove worktree', 'isolate this in a worktree', 'symlink env into worktree'.
  → read `.rig/skills/rig-worktree.md` and follow it.

**Roles/subagents:** personas live in `.rig/agents/` (rig-reviewer, rig-coder,
rig-architect, rig-qa, rig-debugger). If your agent supports subagents,
delegate to the named persona; otherwise adopt that persona's instructions
inline. Helper scripts are in `.rig/scripts/`; review patterns in
`.rig/REVIEWER.md`; kit reference docs in `.rig/docs/`.

`.rig/` is one rig home: the profile (`config.json`, `schema.json`) plus the
agent-neutral view of the same kit Claude Code reads from `.claude/`. The skill
files are flattened copies; `agents/`, `scripts/`, `docs/`, `REVIEWER.md`, and
`label-mapping.md` are symlinks into `.claude/`, so there is one source of
truth per file.
<!-- rig:end -->
