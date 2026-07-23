# PetCard

PetCard is an optional, compile-time PulseDeck card plugin. It displays Codex
lifecycle state using cached image frames and performs no polling while
offline.

## Build

```sh
cargo build --release --features pet-card
```

To build both optional integrations:

```sh
cargo build --release --features scrcpy-forge,pet-card
```

Append `src/plugins/pet_card/config.example.toml` to the local PulseDeck
configuration and update `asset_root`. If no matching image exists, the card
uses an emoji fallback, so the event path can be tested before artwork is
installed.

Set `completion_sound = true` in `[cards.plugin]` to play the desktop theme's
single `complete` event when the state first changes to `done`. It is disabled by
default. Every later transition from an active state to `done` plays once;
repeated reads of the same `done` event do not replay it.

Set `completion_sound_file` to an audio file path to replace the theme event
with a custom sound. Playback uses `canberra-gtk-play` without a shell and is
waited for asynchronously, so it neither blocks state handling nor depends on
GTK input-feedback bells. If the player is unavailable, PetCard falls back to
the GTK bell.

## Codex integration

The installable Codex plugin is under `integrations/pulsedeck-pet`. Its Bash
hook writes only a fixed state, protocol version and timestamp. It deliberately
does not read the event JSON on stdin, so prompt text, commands, tool arguments,
tool output and session ids are neither parsed nor persisted.

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

## Runtime behavior

- State updates use an atomic file replacement and a directory file monitor.
- The current state's frames are decoded on state transitions and retained in
  memory during that state, so animation does not read from disk per frame.
- Animation is capped to 30 FPS and defaults to 12 FPS.
- Frame advancement pauses while the card is not mapped.
- Offline and any single-frame state have no animation timer.
- Stale state automatically returns to offline after
  `offline_after_seconds`.
- After five continuous offline minutes, the card temporarily returns to one
  normal cell. The next active state restores the last user-selected size.
- Size preference writes happen only when the user makes a selection and use
  `${XDG_STATE_HOME:-$HOME/.local/state}/pulsedeck/pet-card-presentation`.

Supported states are `offline`, `ready`, `thinking`, `working`, `coding`,
`waiting`, `error`, and `done`.

## Card size

Long-press PetCard to choose one of four runtime presentation modes:

- **Normal** keeps the card in its original FlowBox slot.
- **Four cells** occupies the left two columns by two rows.
- **Six cells** occupies the left two columns by three rows.
- **Fullscreen** fills the current PulseDeck page below the page switcher.

The four/six-cell calculation follows the current page grid. In the normal
three-column layout, remaining cards fill the right column. In compact
six-column mode, PetCard still spans only the left two columns and remaining
cards fill the other four. Changing the toolbar grid mode immediately reflows
an enlarged PetCard.

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
| `pause_when_unmapped` | `true` | Pause frame advancement while not visible. |
| `show_status` | `true` | Show the state label below the image. |
| `completion_sound` | `false` | Play one sound per completion transition. |
| `completion_sound_file` | unset | Custom audio file; otherwise use the theme's `complete` event. |

Each `[cards.plugin.animations.<state>]` table accepts `frames`, optional `fps`,
and `loop`. Frame paths are resolved under `asset_root`. A single-frame offline
animation is recommended so absent Codex sessions have no animation wakeups.

## Plugin boundary

PulseDeck core owns only generic `kind` and `plugin` configuration fields plus
the page/card plugin traits. Each optional module owns its typed configuration,
dependencies and UI implementation. Disabled Cargo features do not compile or
register their plugin modules.

ScrcpyForge and other page plugins use the generic `[pages.plugin]` table.
