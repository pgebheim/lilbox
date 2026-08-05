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

The live end-to-end paths need a KVM host and network access for image pulls.
