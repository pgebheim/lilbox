#!/usr/bin/env bash
# Tests for contrib/herdr/bin/lilbox-herdr.
#
# The shim is exercised against stub `lilbox` / `herdr` / `fzf` binaries served
# from a fixture state dir, so no KVM host, tailnet, or herdr server is needed.
#
#   contrib/herdr/tests/run.sh
set -euo pipefail

SHIM=$(realpath "$(dirname "$0")/../bin/lilbox-herdr")

PASS=0
FAIL=0
TEST=

ok() { PASS=$((PASS + 1)); printf 'ok %s - %s\n' "$TEST" "$1"; }
bad() {
	FAIL=$((FAIL + 1))
	printf 'not ok %s - %s\n' "$TEST" "$1" >&2
}

assert_contains() { # file substring label
	if grep -qF -- "$2" "$1" 2>/dev/null; then ok "$3"; else
		bad "$3 (wanted '$2' in: $(cat "$1" 2>/dev/null))"
	fi
}

assert_not_contains() { # file substring label
	if grep -qF -- "$2" "$1" 2>/dev/null; then
		bad "$3 (unexpected '$2' in: $(cat "$1")"
	else ok "$3"; fi
}

assert_eq() { # actual expected label
	if [ "$1" = "$2" ]; then ok "$3"; else bad "$3 (got '$1', want '$2')"; fi
}

# --- fixture ----------------------------------------------------------------

TMP=
setup() {
	TMP=$(mktemp -d)
	STUB=$TMP/stub
	export LILBOX_STUB_STATE=$TMP/state
	ALIVE_DIR=$TMP/alive-worktree
	GONE_DIR=$TMP/gone-worktree # deliberately never created
	PSTATE=$TMP/plugin-state
	mkdir -p "$STUB" "$LILBOX_STUB_STATE" "$ALIVE_DIR" "$PSTATE"
	: >"$LILBOX_STUB_STATE/calls.log"

	# `lilbox` stub: serves canned ls/stat, records mutating verbs.
	cat >"$STUB/lilbox" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
s=$LILBOX_STUB_STATE
cmd=${1:-}
[ $# -eq 0 ] || shift
case $cmd in
ls) cat "$s/ls.json" ;;
stat)
	if [ -f "$s/stat-$1.txt" ]; then cat "$s/stat-$1.txt"; else
		echo "no such box: $1" >&2
		exit 1
	fi
	;;
rm)
	echo "rm $*" >>"$s/calls.log"
	if grep -qxF "$1" "$s/rm-fails" 2>/dev/null; then
		echo "cannot remove $1: sandbox busy" >&2
		exit 1
	fi
	;;
stop | start | expose | unexpose | agent) echo "$cmd $*" >>"$s/calls.log" ;;
ssh) echo "ssh $*" >>"$s/calls.log" ;;
*)
	echo "stub lilbox: unexpected call: $cmd $*" >&2
	exit 1
	;;
esac
EOF

	# `herdr` stub: records plugin pane opens.
	cat >"$STUB/herdr" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$LILBOX_STUB_STATE/herdr-calls.log"
EOF

	# `fzf` stub: captures stdin, "selects" the first row.
	cat >"$STUB/fzf" <<'EOF'
#!/usr/bin/env bash
cat >"$LILBOX_STUB_STATE/fzf-in"
head -n1 "$LILBOX_STUB_STATE/fzf-in"
EOF
	chmod +x "$STUB/lilbox" "$STUB/herdr" "$STUB/fzf"

	cat >"$LILBOX_STUB_STATE/ls.json" <<EOF
[
  {"name":"hd-alive-aaa111","image":"python","status":"running","guest_port":null,"host_port":null,"serve_port":null,"public":false,"url":null,"tailnet_url":"https://alive.ts.net","created":"2026-08-09T00:00:00Z"},
  {"name":"hd-gone-bbb222","image":"lilbox-box","status":"stopped","guest_port":null,"host_port":null,"serve_port":null,"public":false,"url":null,"tailnet_url":null,"created":"2026-08-09T00:01:00Z"},
  {"name":"scratch","image":"python","status":"running","guest_port":8080,"host_port":18080,"serve_port":null,"public":false,"url":"http://localhost:18080","tailnet_url":null,"created":"2026-08-09T00:02:00Z"}
]
EOF

	# `lilbox stat` fixtures. The config block is the persisted microsandbox
	# SandboxConfig; mounts serialize as externally-tagged VolumeMounts.
	cat >"$LILBOX_STUB_STATE/stat-hd-alive-aaa111.txt" <<EOF
name: hd-alive-aaa111
status: running
tailscale node: -
config: {
  "name": "hd-alive-aaa111",
  "mounts": [
    {"Bind": {"host": "$ALIVE_DIR", "guest": "/workspace"}},
    {"Named": {"name": "hd-alive-aaa111-home", "guest": "/root"}}
  ]
}
EOF
	cat >"$LILBOX_STUB_STATE/stat-hd-gone-bbb222.txt" <<EOF
name: hd-gone-bbb222
status: stopped
tailscale node: -
config: {
  "name": "hd-gone-bbb222",
  "mounts": [
    {"Bind": {"host": "$GONE_DIR", "guest": "/workspace"}}
  ]
}
EOF
	cat >"$LILBOX_STUB_STATE/stat-scratch.txt" <<EOF
name: scratch
status: running
tailscale node: -
config: {
  "name": "scratch",
  "mounts": []
}
EOF
}

teardown() {
	[ -n "$TMP" ] && rm -rf "$TMP"
	TMP=
}

# Run the shim against the fixture.
shim() {
	env \
		PATH="$STUB:/usr/bin:/bin" \
		LILBOX_BIN=lilbox \
		HERDR_BIN_PATH="$STUB/herdr" \
		"$SHIM" "$@"
}

# --- tests ------------------------------------------------------------------

TEST=1 # manage-rows formats boxes and flags stale worktrees
setup
out=$TMP/out
shim manage-rows >"$out"
assert_contains "$out" "hd-alive-aaa111" "lists running box"
assert_contains "$out" "https://alive.ts.net" "prefers tailnet URL"
assert_contains "$out" "http://localhost:18080" "falls back to forwarded URL"
assert_contains "$out" "path gone" "flags box whose worktree is gone"
line_alive=$(grep hd-alive "$out")
case $line_alive in *"path gone"*) bad "live worktree not flagged" ;; *) ok "live worktree not flagged" ;; esac
line_scratch=$(grep scratch "$out")
case $line_scratch in *"path gone"*) bad "box without /workspace mount never stale" ;; *) ok "box without /workspace mount never stale" ;; esac
teardown

TEST=2 # workspace-source reads the /workspace bind host out of lilbox stat
setup
assert_eq "$(shim workspace-source hd-gone-bbb222)" "$GONE_DIR" "extracts bind host"
assert_eq "$(shim workspace-source scratch)" "" "empty when no /workspace mount"
teardown

TEST=3 # gc destroys exactly the stale boxes
setup
out=$(shim gc)
calls=$(cat "$LILBOX_STUB_STATE/calls.log")
assert_eq "$calls" "rm hd-gone-bbb222" "only the stale box is destroyed"
case $out in *"destroyed hd-gone-bbb222"*) ok "reports the destroy" ;; *) bad "reports the destroy (got: $out)" ;; esac
teardown

TEST=4 # gc --dry-run destroys nothing
setup
out=$(shim gc --dry-run)
assert_eq "$(cat "$LILBOX_STUB_STATE/calls.log")" "" "no rm calls"
case $out in *"hd-gone-bbb222"*) ok "lists the stale box" ;; *) bad "lists the stale box (got: $out)" ;; esac
teardown

TEST=5 # gc reports rm failures and exits nonzero, but keeps going
setup
echo "hd-gone-bbb222" >"$LILBOX_STUB_STATE/rm-fails"
rc=0
err=$TMP/err
shim gc 2>"$err" >/dev/null || rc=$?
assert_eq "$rc" "1" "exit 1 when a destroy fails"
assert_contains "$err" "retry: lilbox rm hd-gone-bbb222" "warns with the retry command"
teardown

TEST=6 # manage-act stop stops the named box (no worktree context needed)
setup
shim manage-act stop hd-alive-aaa111
assert_eq "$(cat "$LILBOX_STUB_STATE/calls.log")" "stop hd-alive-aaa111" "lilbox stop called"
teardown

TEST=7 # manage-act expose-toggle flips on tailnet exposure
setup
shim manage-act expose-toggle hd-alive-aaa111 # has tailnet_url -> unexpose
shim manage-act expose-toggle scratch         # no tailnet_url -> expose
assert_eq "$(cat "$LILBOX_STUB_STATE/calls.log")" "unexpose hd-alive-aaa111
expose scratch" "toggle picks the right direction"
teardown

TEST=8 # manage hands the picked box to the box pane entrypoint
setup
shim_out=$TMP/out
HERDR_PLUGIN_ID=lilbox HERDR_PLUGIN_STATE_DIR=$PSTATE LILBOX_FZF_BIN="$STUB/fzf" \
	shim manage >"$shim_out"
assert_eq "$(cat "$PSTATE/pending-box")" "hd-alive-aaa111" "pending box recorded"
assert_contains "$LILBOX_STUB_STATE/herdr-calls.log" \
	"plugin pane open --plugin lilbox --entrypoint box" "box entrypoint opened"
teardown

TEST=9 # open wakes a stopped pending box before attaching
setup
printf 'hd-gone-bbb222' >"$PSTATE/pending-box"
HERDR_PLUGIN_ID=lilbox HERDR_PLUGIN_STATE_DIR=$PSTATE shim open
assert_eq "$(cat "$LILBOX_STUB_STATE/calls.log")" "start hd-gone-bbb222
ssh hd-gone-bbb222 -- sh -lc cd /workspace 2>/dev/null || true; exec bash -l" \
	"starts the stopped box, then attaches"
teardown

TEST=10 # manage without fzf fails with a friendly message
setup
rc=0
err=$TMP/err
LILBOX_FZF_BIN=definitely-not-fzf shim manage 2>"$err" >/dev/null || rc=$?
[ "$rc" -ne 0 ] && ok "exits nonzero" || bad "exits nonzero"
assert_contains "$err" "fzf" "message names fzf"
teardown

TEST=11 # manage with no boxes says so and exits cleanly
setup
echo '[]' >"$LILBOX_STUB_STATE/ls.json"
out=$(LILBOX_FZF_BIN="$STUB/fzf" shim manage)
case $out in *"no boxes"*) ok "prints no-boxes message" ;; *) bad "prints no-boxes message (got: $out)" ;; esac
teardown

# --- summary ----------------------------------------------------------------
echo
if [ "$FAIL" -gt 0 ]; then
	printf '%d passed, %d FAILED\n' "$PASS" "$FAIL" >&2
	exit 1
fi
printf 'all %d assertions passed\n' "$PASS"
