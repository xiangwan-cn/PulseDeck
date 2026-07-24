# OpenCode PetCard Integration

This zero-dependency OpenCode plugin publishes the same privacy-minimized state
file as the Codex integration. It reacts only to lifecycle event names and does
not read or retain prompts, commands, tool arguments, tool output, or real
session identifiers.

Install it globally:

```sh
install -Dm600 integrations/pulsedeck-pet/opencode/pulsedeck-pet.ts \
  "$HOME/.config/opencode/plugins/pulsedeck-pet.ts"
```

Quit and restart OpenCode after installing or updating the plugin. OpenCode
automatically loads TypeScript files under `~/.config/opencode/plugins/`; no
`opencode.jsonc` entry is required.

The default state file is:

```text
$XDG_RUNTIME_DIR/pulsedeck/codex-pet.json
```

Set `PULSEDECK_PET_STATE_FILE` for OpenCode and `state_file` in
`[cards.plugin]` when a custom path is required. State writes are serialized
and atomically replace the target file. Repeated activity is rate-limited to an
event-driven heartbeat at most once per minute; no polling timer or helper
process is started.

The state file is a single shared slot. If Codex and OpenCode run concurrently,
the most recent lifecycle event is displayed.
