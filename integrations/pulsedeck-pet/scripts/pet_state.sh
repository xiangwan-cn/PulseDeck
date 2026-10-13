#!/bin/sh
# Publish one fixed, privacy-minimized state. For PreToolUse, inspect only the
# tool_name field to distinguish edits; never retain or write event contents.
# Drain other hook input so Codex never sees a broken pipe.
# POSIX sh (dash/busybox ash) compatible so no external bash dependency is needed.

set -u
umask 077

action=${1:-working}
tool_name=
if [ "$action" = "tool" ]; then
  if command -v jq >/dev/null 2>&1; then
    tool_name=$(jq -r '.tool_name // empty' 2>/dev/null) || tool_name=
  else
    while IFS= read -r _hook_line; do :; done
  fi
  case "$tool_name" in
    apply_patch|Edit|Write) action=coding ;;
    *) action=working ;;
  esac
else
  # Codex writes event JSON to every command hook. Drain it without storing it.
  while IFS= read -r _hook_line; do :; done
fi

case "$action" in
  ready|start|thinking|working|coding|waiting|confirm|cancelled|aborted|error|done|offline) ;;
  *) action=working ;;
esac

if [ -n "${PULSEDECK_PET_STATE_FILE:-}" ]; then
  target=$PULSEDECK_PET_STATE_FILE
elif [ -n "${XDG_RUNTIME_DIR:-}" ]; then
  target=$XDG_RUNTIME_DIR/pulsedeck/codex-pet.json
else
  target=/tmp/pulsedeck-$(id -u)/pulsedeck/codex-pet.json
fi

case "$target" in
  */*) directory=${target%/*}; directory=${directory:-/} ;;
  *) directory=. ;;
esac
mkdir -p -m 700 "$directory" 2>/dev/null || exit 0
meta=$directory/.codex-pet-meta
task_id=none
event_counter=0
last_state=offline
last_write=0
if [ -r "$meta" ]; then
  IFS=' ' read -r task_id event_counter last_state last_write <"$meta" || true
fi

# A killed/concurrent hook can leave old or malformed metadata behind. Never
# let shell arithmetic turn that recoverable cache issue into a Codex hook
# failure.
case "$event_counter" in ''|*[!0-9]*) event_counter=0 ;; esac
case "$last_write" in ''|*[!0-9]*) last_write=0 ;; esac
# Bound numeric metadata before shell arithmetic (even digit-only corrupt
# values can overflow the shell's integer representation).
[ "${#event_counter}" -le 9 ] || event_counter=0
[ "${#last_write}" -le 12 ] || last_write=0
# Leading zeroes are not octal: normalize cached numbers before arithmetic.
while [ "${event_counter#0}" != "$event_counter" ]; do event_counter=${event_counter#0}; done
while [ "${last_write#0}" != "$last_write" ]; do last_write=${last_write#0}; done
event_counter=${event_counter:-0}
last_write=${last_write:-0}
case "$task_id" in ''|*[!A-Za-z0-9._-]*) task_id=none ;; esac
case "$last_state" in
  ready|thinking|working|coding|waiting|confirm|cancelled|aborted|error|done|offline) ;;
  *) last_state=offline ;;
esac

now_seconds=$(date +%s 2>/dev/null || echo 0)
case "$now_seconds" in ''|*[!0-9]*) now_seconds=0 ;; esac
timestamp_ms=$((now_seconds * 1000))
state=$action
important=0
case "$action" in
  start)
    task_id="${now_seconds}-$$"
    state=thinking
    # A new task must be published even if the previous one was thinking.
    important=1
    ;;
  waiting|confirm|cancelled|aborted|error|done)
    # Repeated delivery of the same terminal/attention hook is one event edge.
    # A later different state (or a new task) makes the same event type valid
    # again.
    [ "$action" = "$last_state" ] && exit 0
    event_counter=$((event_counter + 1))
    important=1
    ;;
esac

# Tool hooks may fire many times in the same state. Keep task liveness as a
# separate, rate-limited heartbeat instead of rewriting the state file per tool.
if [ "$state" = "$last_state" ] && [ "$important" -eq 0 ] && [ $((now_seconds - last_write)) -lt 60 ]; then
  exit 0
fi

temporary=$directory/.codex-pet.$$.tmp
meta_temporary=$directory/.codex-pet-meta.$$.tmp
trap 'test ! -e "$temporary" || unlink "$temporary"; test ! -e "$meta_temporary" || unlink "$meta_temporary"' EXIT
printf '{"version":2,"task_id":"%s","event_id":"%s","state":"%s","timestamp_ms":%s}\n' \
  "$task_id" "$event_counter" "$state" "$timestamp_ms" >"$temporary" || exit 0
mv -f "$temporary" "$target" 2>/dev/null || exit 0
printf '%s %s %s %s\n' "$task_id" "$event_counter" "$state" "$now_seconds" \
  >"$meta_temporary" || exit 0
mv -f "$meta_temporary" "$meta" 2>/dev/null || exit 0
trap - EXIT
exit 0
