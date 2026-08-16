#!/bin/sh
# Publish one fixed, privacy-minimized state. Drain Codex event JSON directly
# to /dev/null so prompts, commands and tool output are never parsed or retained.
# POSIX sh (dash/busybox ash) compatible so no external bash dependency is needed.

set -u
umask 077

# Codex writes the event payload to every command hook. Exiting before consuming
# it closes the pipe early and makes the caller report EPIPE/Broken pipe.
while IFS= read -r _hook_line; do :; done

action=${1:-working}
case "$action" in
  ready|start|thinking|working|coding|waiting|confirm|cancelled|aborted|error|done) ;;
  *) action=working ;;
esac

if [ -n "${PULSEDECK_PET_STATE_FILE:-}" ]; then
  target=$PULSEDECK_PET_STATE_FILE
elif [ -n "${XDG_RUNTIME_DIR:-}" ]; then
  target=$XDG_RUNTIME_DIR/pulsedeck/codex-pet.json
else
  target=/tmp/pulsedeck-$(id -u)/pulsedeck/codex-pet.json
fi

directory=${target%/*}
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
