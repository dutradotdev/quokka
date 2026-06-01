#!/usr/bin/env bash
#
# tmux driver for quokka's /qa real-device E2E layer (QA strategy, layer 5).
#
# This file owns ONLY the deterministic mechanics: start the real binary in a
# fixed-size tmux pane, inject keystrokes, wait for expected text, and capture
# the rendered frame as plain text. It makes NO pass/fail decisions — the
# caller (the /qa skill, or a future standalone runner) greps the captured
# text, compares golden frames, and checks exit codes.
#
# A fixed pane size is mandatory: it makes ratatui layout deterministic, which
# is what lets captured frames double as golden snapshots.
#
# Usage:
#   drive.sh platforms                  # list attached platforms (ios/android)
#   drive.sh run <args...>              # run a non-interactive command to completion
#   drive.sh start <session> <args...>  # launch an interactive command, detached
#   drive.sh keys  <session> <key...>   # send keystrokes (Down, Enter, Space, q, "text")
#   drive.sh wait  <session> <text> [timeout_s]   # poll until <text> appears
#   drive.sh capture <session>          # print the current rendered frame
#   drive.sh stop  <session>            # tear the session down
#
# Env: QA_COLS (200), QA_ROWS (50), QA_TIMEOUT (30), QK_BIN (target/debug/qk).
set -euo pipefail

QA_COLS="${QA_COLS:-200}"
QA_ROWS="${QA_ROWS:-50}"
QA_TIMEOUT="${QA_TIMEOUT:-30}"

repo_root() { cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd; }
QK_BIN="${QK_BIN:-$(repo_root)/target/debug/qk}"

die() {
  echo "drive.sh: $*" >&2
  exit 1
}

# Strip a leading `qk`/`quokka` token so YAML `command: qk status` and a bare
# `status` both work.
strip_bin_prefix() {
  case "${1:-}" in
  qk | quokka)
    shift
    ;;
  esac
  printf '%s\n' "$@"
}

require_tmux() { command -v tmux >/dev/null 2>&1 || die "tmux not installed (brew install tmux)"; }
require_bin() { [ -x "$QK_BIN" ] || die "binary not found at $QK_BIN — run 'cargo build' first"; }

cmd_platforms() {
  require_bin
  if command -v jq >/dev/null 2>&1; then
    "$QK_BIN" devices --json 2>/dev/null | jq -r '.[].platform' 2>/dev/null | sort -u
  else
    # Fallback: scrape the JSON without jq.
    "$QK_BIN" devices --json 2>/dev/null | grep -o '"platform": *"[a-z]*"' |
      sed 's/.*"\([a-z]*\)"$/\1/' | sort -u
  fi
}

# Run a non-interactive command to completion. Prints the captured frame, then
# a trailing `__QA_EXIT__=<code>` line the caller parses for the exit status.
cmd_run() {
  require_tmux
  require_bin
  # shellcheck disable=SC2046
  set -- $(strip_bin_prefix "$@")
  local session="qa_run_$$_${RANDOM}"
  local rc_file chan
  rc_file="$(mktemp)"
  chan="qa_done_${session}"
  tmux kill-session -t "$session" 2>/dev/null || true
  tmux new-session -d -s "$session" -x "$QA_COLS" -y "$QA_ROWS"
  # Run, stash the exit code, then signal completion over a tmux channel so we
  # block precisely until the command finishes — no arbitrary sleep. We do NOT
  # set QK_NON_INTERACTIVE: layer 5 exists to exercise the real TTY behavior.
  tmux send-keys -t "$session" \
    "'$QK_BIN' $* ; echo \$? > '$rc_file' ; tmux wait -S '$chan'" Enter
  timeout "$QA_TIMEOUT" tmux wait "$chan" 2>/dev/null || true
  tmux capture-pane -t "$session" -p -S -5000 | sed -e 's/[[:space:]]*$//'
  local code
  code="$(cat "$rc_file" 2>/dev/null || echo "timeout")"
  echo "__QA_EXIT__=${code}"
  rm -f "$rc_file"
  tmux kill-session -t "$session" 2>/dev/null || true
}

cmd_start() {
  require_tmux
  require_bin
  local session="$1"
  shift
  # shellcheck disable=SC2046
  set -- $(strip_bin_prefix "$@")
  tmux kill-session -t "$session" 2>/dev/null || true
  tmux new-session -d -s "$session" -x "$QA_COLS" -y "$QA_ROWS"
  tmux send-keys -t "$session" "'$QK_BIN' $*" Enter
}

cmd_keys() {
  require_tmux
  local session="$1"
  shift
  local key
  for key in "$@"; do
    tmux send-keys -t "$session" -- "$key"
  done
}

cmd_wait() {
  require_tmux
  local session="$1" needle="$2" timeout="${3:-$QA_TIMEOUT}"
  local waited=0
  while [ "$waited" -lt "$timeout" ]; do
    if tmux capture-pane -t "$session" -p 2>/dev/null | grep -qE -- "$needle"; then
      return 0
    fi
    sleep 0.25
    waited=$((waited + 1))
  done
  die "timed out after ${timeout}s waiting for /$needle/ in session $session"
}

cmd_capture() {
  require_tmux
  tmux capture-pane -t "$1" -p -S -5000 | sed -e 's/[[:space:]]*$//'
}

cmd_stop() {
  require_tmux
  tmux kill-session -t "$1" 2>/dev/null || true
}

main() {
  [ $# -ge 1 ] || die "no subcommand (try: platforms|run|start|keys|wait|capture|stop)"
  local sub="$1"
  shift
  case "$sub" in
  platforms) cmd_platforms "$@" ;;
  run) cmd_run "$@" ;;
  start) cmd_start "$@" ;;
  keys) cmd_keys "$@" ;;
  wait) cmd_wait "$@" ;;
  capture) cmd_capture "$@" ;;
  stop) cmd_stop "$@" ;;
  *) die "unknown subcommand: $sub" ;;
  esac
}

main "$@"
