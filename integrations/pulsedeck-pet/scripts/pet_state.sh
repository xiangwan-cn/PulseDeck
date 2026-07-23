#!/usr/bin/env bash
# Publish one fixed, privacy-minimized state. Codex event JSON on stdin is
# intentionally ignored, so prompts, commands and tool output are never parsed.

set -u

state=${1:-working}
case "$state" in
  ready|thinking|working|coding|waiting|error|done) ;;
  *) state=working ;;
esac

if [[ -n ${PULSEDECK_PET_STATE_FILE:-} ]]; then
  target=$PULSEDECK_PET_STATE_FILE
elif [[ -n ${XDG_RUNTIME_DIR:-} ]]; then
  target=$XDG_RUNTIME_DIR/pulsedeck/codex-pet.json
else
  target=/tmp/pulsedeck-$(id -u)/pulsedeck/codex-pet.json
fi

directory=${target%/*}
mkdir -p -m 700 "$directory" 2>/dev/null || exit 0
temporary=$(mktemp "$directory/.codex-pet.XXXXXX") || exit 0
trap 'test ! -e "$temporary" || unlink "$temporary"' EXIT

timestamp_ms=$(($(date +%s) * 1000))
printf '{"version":1,"state":"%s","timestamp_ms":%s}\n' \
  "$state" "$timestamp_ms" >"$temporary" || exit 0
chmod 600 "$temporary" 2>/dev/null || exit 0
mv -f "$temporary" "$target" 2>/dev/null || exit 0
trap - EXIT
exit 0
