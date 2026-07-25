# pi PetCard Integration

This zero-dependency pi extension publishes the same privacy-minimized state
file as the Codex and OpenCode integrations. It reacts only to lifecycle event
names and never reads or retains prompts, messages, tool arguments, commands,
or tool output.

Install it globally:

```sh
install -Dm600 integrations/pulsedeck-pet/pi/pulsedeck-pet.ts \
  "$HOME/.pi/agent/extensions/pulsedeck-pet.ts"
```

Run `/reload` in pi after installing or updating the extension, or restart pi.
A project-local installation under `.pi/extensions/` is also supported, but pi
loads it only after the project is trusted.

The extension maps session startup to `ready`, a new agent run to `thinking`,
tool execution to `working`, a fully settled run to `done`, and session
shutdown to `offline`. Streaming activity only produces an event-driven,
rate-limited heartbeat at most once per minute; it starts no timer, watcher,
subprocess, or resident helper.

The default state file is:

```text
$XDG_RUNTIME_DIR/pulsedeck/codex-pet.json
```

Set `PULSEDECK_PET_STATE_FILE` for pi and `state_file` in `[cards.plugin]` when
a custom path is required. Writes are serialized and atomically replace the
target file.

The state file is a single shared slot. If Codex, OpenCode, and pi run
concurrently, the most recent lifecycle event is displayed.
