# PetCard

PetCard is an optional, compile-time PulseDeck card plugin. It displays Codex,
OpenCode, or pi lifecycle state using cached image frames and performs no
polling while offline.

The generic `[cards.display.colors]`, `[[cards.display.states]]`, and
`[cards.display.transition]` options are intentionally limited to standard
non-plugin cards. A PetCard declares `kind = "pet-card"`, so its lifecycle
artwork, state labels, transitions, and colors remain owned by the plugin
configuration documented here and are never rewritten by generic card rules.
The PetCard example is a fragment to append to a strict schema v2 PulseDeck
configuration; it is not a standalone legacy configuration file.

## Build

```sh
cargo build --release --features pet-card
```

To build both optional integrations:

```sh
cargo build --release --features scrcpy-forge,pet-card
```

When the `pet-card` feature is present, PulseDeck automatically adds an enabled
`codex-pet` card to a config that does not already contain one. It uses safe
runtime defaults and emoji artwork without requiring AI or a manual config
edit. To install custom artwork, copy the animation options from
`src/plugins/pet_card/config.example.toml` and update `asset_root`.

Set `codex_completion_sound = true` in the global `[runtime]` section to play
the desktop theme's single `complete` event for a completion, failure,
cancellation, waiting-input, confirmation-required, or abnormal-stop edge.
Task and event identifiers deduplicate notifications, so polling or rereading
the same state cannot replay the sound. The default example enables it.

Set `completion_sound_file` to an audio file path to replace the theme event
with a custom sound. Playback uses `canberra-gtk-play` without a shell and is
waited for asynchronously, so it neither blocks state handling nor depends on
GTK input-feedback bells. If the player is unavailable, PetCard falls back to
the GTK bell.

## Codex integration

The installable Codex plugin is under `integrations/pulsedeck-pet`. Its Bash
hook writes only a fixed state, protocol version and timestamp. A POSIX shell
read loop drains the event JSON from stdin without parsing it, so prompt text,
commands, tool arguments, tool output and session ids are neither retained nor
passed through an extra helper process. Corrupt or partial heartbeat metadata
is reset instead of surfacing as a hook command error.
Draining prevents Codex from seeing a broken pipe after the short-lived hook
publishes its state.

Codex must trust the plugin hooks before they run. Inspect them with `/hooks`
after installing or enabling the plugin.

For a personal Codex marketplace, register this directory as
`pulsedeck-pet`, install it with:

```sh
codex plugin add pulsedeck-pet@<marketplace>
```

Then start a new Codex thread so the hook set is loaded. The plugin manifest is
`integrations/pulsedeck-pet/.codex-plugin/plugin.json`; the hook and its
privacy-minimized Bash writer remain inside the same integration directory.

The default state file is:

```text
$XDG_RUNTIME_DIR/pulsedeck/codex-pet.json
```

Set `PULSEDECK_PET_STATE_FILE` for the hook and `state_file` in
`[cards.plugin]` when a custom path is required.

## OpenCode integration

OpenCode can publish the same state protocol through the zero-dependency local
plugin at `integrations/pulsedeck-pet/opencode/pulsedeck-pet.ts`. Install it to
the global auto-discovery directory:

```sh
install -Dm600 integrations/pulsedeck-pet/opencode/pulsedeck-pet.ts \
  "$HOME/.config/opencode/plugins/pulsedeck-pet.ts"
```

Quit and restart OpenCode after installation. The plugin maps lifecycle events
to fixed PetCard states without reading prompts, commands, tool arguments, tool
output, or real session identifiers. It writes atomically, uses event-driven
rate-limited heartbeats, and starts no polling timer or helper process. Codex
and OpenCode share one state file, so the most recent event wins when both are
active.

## pi integration

pi can publish the same state protocol through the zero-dependency extension at
`integrations/pulsedeck-pet/pi/pulsedeck-pet.ts`. Install it globally with:

```sh
install -Dm600 integrations/pulsedeck-pet/pi/pulsedeck-pet.ts \
  "$HOME/.pi/agent/extensions/pulsedeck-pet.ts"
```

Run `/reload` or restart pi after installation. The extension observes only pi
lifecycle event names: session startup, agent startup, tool execution, settled
completion, and session shutdown. It does not read prompts, messages, tool
arguments, commands, or tool output. Streaming activity creates only a
rate-limited, event-driven heartbeat and starts no timer or helper process.
See `integrations/pulsedeck-pet/pi/README.md` for state mapping and custom path
details. Codex, OpenCode, and pi share one state file, so the latest event wins
when clients run concurrently.

## Runtime behavior

- State updates use an atomic file replacement and a directory file monitor.
- The current state's frames are decoded on state transitions and retained in
  memory during that state, so animation does not read from disk per frame.
- Animation is capped to 12 FPS and defaults to 12 FPS.
- Frame timers are removed, rather than callback-skipped, while the card is
  unmapped or PulseDeck is in the background.
- Foreground idle mode reduces animation to 1 FPS; normal and Codex protection
  modes use the configured rate.
- Offline and any single-frame state have no animation timer.
- Stale state automatically returns to offline after
  `offline_after_seconds`.
- A stale state only changes the card's visual state. It does not cancel an
  already active agent's original one-hour idle-overlay protection; an explicit
  terminal/offline event or the protection deadline controls that lifecycle.
- After five continuous offline minutes, the card temporarily returns to one
  normal cell. The next active state restores the last user-selected size.
- Size preference writes happen only when the user makes a selection and use
  `${XDG_STATE_HOME:-$HOME/.local/state}/pulsedeck/pet-card-presentation`.

Supported states include `offline`, `ready`, `thinking`, `working`, `coding`,
`waiting`, `confirm`, `error`, `cancelled`, `aborted`, and `done`.

## Card size

Double-click PetCard to cycle quickly through the four runtime presentation
modes. Long-press it to choose a specific mode directly:

- **Normal** keeps the card in its original FlowBox slot.
- **Four cells** occupies the left two columns by two rows.
- **Six cells** occupies the left two columns by three rows.
- **Fullscreen** fills the current PulseDeck page below the page switcher.

The four/six-cell calculation follows the current page grid. In the normal
three-column layout, remaining cards fill the right column. In compact
six-column mode, PetCard still spans only the left two columns and remaining
cards fill the other four. Both layouts derive three row heights from the
currently visible page, so the enlarged PetCard and its companion cards fill
the same 3×3 or 6×3 viewport. Changing the toolbar grid mode immediately
reflows an enlarged PetCard.

Use the restore button or `Escape` to leave fullscreen. Presentation preference
is persisted outside `config.toml`. Offline fallback does not overwrite that
preference.

## Configuration reference

The example at `src/plugins/pet_card/config.example.toml` is authoritative.
Important plugin options are:

| Option | Default | Behavior |
| --- | ---: | --- |
| `offline_after_seconds` | `180` | Age after which a Codex state is stale. |
| `offline_normal_after_seconds` | `300` | Continuous offline time before temporary one-cell fallback. |
| `fps` | `12` | Default animation rate; per-state `fps` overrides it. |
| `done_hold_seconds` | `5` | Time the `done` animation remains before `ready`. |
| `pause_when_unmapped` | `true` | Remove the frame timer while not visible. |
| `show_status` | `true` | Show the state label below the image. |
| `completion_sound_file` | unset | Custom audio file; otherwise use the theme's `complete` event. |

Sound enable/disable, the new-task brightness-protection period, and the
post-event attention period belong to `[runtime]`, because they affect the
whole application. PetCard only owns its optional custom audio asset.

Each `[cards.plugin.animations.<state>]` table accepts `frames`, optional `fps`,
and `loop`. Frame paths are resolved under `asset_root`. A single-frame offline
animation is recommended so absent Codex sessions have no animation wakeups.

## Plugin boundary

PulseDeck core owns only generic `kind` and `plugin` configuration fields plus
the page/card plugin traits. Each optional module owns its typed configuration,
dependencies and UI implementation. Disabled Cargo features do not compile or
register their plugin modules.

ScrcpyForge and other page plugins use the generic `[pages.plugin]` table.
Both plugin types receive the same generic runtime handle for mode changes,
real user activity, bounded interaction leases, and important events; plugins
do not inspect system power or control global brightness themselves.
