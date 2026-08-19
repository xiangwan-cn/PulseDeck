# PulseDeck

[简体中文](README_CN.md) | English

PulseDeck is a lightweight, configuration-driven GTK4/Libadwaita dashboard for
Linux phones, tablets, and desktops. Pages, metric cards, refresh schedules,
parsers, and actions are described in TOML or JSON, so most dashboard changes
do not require recompiling the application.

![PulseDeck default dashboard](docs/images/pulsedeck-default.png)

_Default build with no optional Cargo features in dark mode: normal layout
(left) and compact layout (right)._

## Features

- Native CPU, memory, battery, power, network, uptime, filesystem, process,
  load, swap, temperature, and network-throughput metrics.
- Built-in, file, command, HTTP, and static-value card sources.
- Value, progress, status, text, list, composite, and action renderers.
- Ordered visual-state rules for standard cards, with numeric/text/source-state
  matching, label and icon overrides, per-region colors, multi-color backgrounds,
  and timer-free color transitions.
- Consistent human-readable primary values: compact percentages, natural unit
  spacing, IP-first network cards, and power-first battery summaries.
- Fixed intervals or schedules such as `daily@08:00,20:00`, with per-slot cache.
- Global and per-card responsive sizing for mobile and desktop layouts.
- Page lifecycle awareness: hidden pages stop polling.
- Unified foreground, idle-power, external-power, background, and Codex
  attention state, with live settings and an application-only dim/minimal mode.
- Event-driven file and network-status cards, coalesced refresh deadlines,
  shared system snapshots, and deduplicated persistent cache writes.
- Bounded subprocess output, HTTP response size, and execution time.
- Optional, separately compiled ScrcpyForge device-control page.
- Optional, separately compiled event-driven Codex/OpenCode/pi PetCard with
  animated lifecycle states, remembered presentation, and completion sound.
- A page-wide toolbar toggle between the configured normal grid and a compact
  six-column grid, with the last choice remembered across launches.

## Runtime and low-power modes

PulseDeck has one event-driven runtime manager shared by ordinary cards and
optional plugins. Only real input such as a click, touch, key press, scroll,
page change, or manual refresh resets user-idle time; automatic refreshes,
animations, file events, and network responses do not.
When conditions overlap, priority is background, external power, important
agent attention, new-task protection, stable idle, then foreground normal.

| Mode | Entry condition | Display and work policy |
| --- | --- | --- |
| Foreground normal | The window is mapped and no higher-priority mode applies. | Uses configured card schedules, normal animation rates, and full plugin presentation. |
| Idle power saving | No real input for `idle_timeout_seconds`, followed by `idle_stability_seconds`. | Throttles refresh by card cost, reduces PetCard to 1 FPS, and lets ScrcpyForge request metadata without preview frames. The `dim` or OLED-friendly `minimal` overlay affects PulseDeck only; it never changes system brightness. |
| External-power realtime | A power supply reports online and `external_realtime` is enabled. | Eligible cards may refresh faster and external power may prevent idle. Command and HTTP cards keep their original interval unless their card policy explicitly opts in. PulseDeck never changes the CPU governor. |
| Agent protection / attention | A new agent task starts, or a distinct completion, failure, cancellation, waiting-input, confirmation, or abort event arrives. | A new task keeps normal visual brightness for its original protection deadline; waiting does not extend it. An important event may play one sound and restore the normal display/refresh policy for the configured attention window. |
| Background | The application window is unmapped. | Releases the screen inhibitor, pauses ordinary card work, removes PetCard frame timers, and stops ScrcpyForge preview work. Fixed lifecycle monitoring and configured notifications remain available. |

Warm or hotter thermal diagnostics reduce expensive plugin presentation work
without changing the external-power verdict: ScrcpyForge slows preview/health
updates, while hot or throttled states freeze PetCard on its current frame.
Any real input restores the foreground UI immediately.

## Page layout modes

The grid button at the right of the page toolbar controls the generic
metric/action card layout:

| Layout | Behavior |
| --- | --- |
| Normal | Uses `[ui].card_columns` (three by default). Card widths share the row and card heights adapt to fit three rows in the visible page. |
| Compact | Reflows metric and action cards into six columns while retaining three fitted rows, with denser padding, typography, and controls. |

This toolbar choice is stored under
`${XDG_STATE_HOME:-$HOME/.local/state}/pulsedeck/compact-grid` and restored on
the next launch. It is a page-grid preference, not a PetCard presentation
choice. Switching it immediately reflows an enlarged PetCard, whose own
normal/four-cell/six-cell/fullscreen preference is described below. Global and
per-card `card_height` values remain minimum heights for small windows or
deliberately taller cards; explicit widths remain supported.

## Requirements

- Linux with GTK 4.10 or newer and Libadwaita 1.2 or newer.
- Rust stable and the native build dependencies required by GTK Rust bindings.
- Optional commands or services referenced by your own configuration.

For Debian-family distributions, the development packages are commonly named
`libgtk-4-dev`, `libadwaita-1-dev`, `pkg-config`, and `build-essential`.
Distribution package names may differ.

## Build and run

```sh
git clone https://github.com/xiangwan-cn/PulseDeck.git
cd PulseDeck
cargo build --release
./target/release/pulsedeck
```

To include the optional ScrcpyForge page:

```sh
cargo build --release --features scrcpy-forge
```

To include PetCard, or both optional integrations:

```sh
cargo build --release --features pet-card
cargo build --release --features scrcpy-forge,pet-card
```

For opt-in internal wakeup/I/O counters used during power profiling:

```sh
cargo build --release --features power-debug
```

## Configuration

On first launch PulseDeck copies the bundled example to:

```text
${XDG_CONFIG_HOME:-$HOME/.config}/pulsedeck/config.toml
```

PulseDeck also scans the adjacent `config.d/` directory automatically. Each
top-level `.toml` or `.json` file there is a standalone module containing
pages, cards, actions, or an explicit named override. Files are loaded in
lexical file-name order; subdirectories and other extensions are ignored. This
makes a card or page exportable by copying one file, with no include list to
maintain. Rename a module to `.disabled` to turn it off.

Configuration uses the strict, non-migrating schema v2. `schema_version = 2`
is required at the document root; unknown fields, obsolete aliases, and unknown
enum values reject the configuration instead of being ignored. Repository
examples and the active local configuration must be updated together whenever
the schema changes.

Start with [config/config.example.toml](config/config.example.toml). A matching
JSON example is available at [config/config.example.json](config/config.example.json).
The current TOML schema is documented with practical card recipes in
[config/CARD_GUIDE.md](config/CARD_GUIDE.md).
PetCard build, hook, animation, sizing, power, and sound behavior is documented
in [docs/PET_CARD.md](docs/PET_CARD.md).
Runtime modes, scheduler policy, plugin integration, and measurement guidance
are documented in [docs/RUNTIME_POWER.md](docs/RUNTIME_POWER.md).

The top-level sections are:

- `schema_version`: required configuration interface version; currently `2`.
- `[app]`: title, logging, output limits, and config reload.
- `[runtime]`: foreground inhibition, low-power display/refresh policy,
  external-power behavior, and agent protection/notification policy.
- `[ui]`: default page plus normal-grid columns and card dimensions; the live
  normal/compact toolbar choice is stored separately as UI state.
- `[[pages]]`: ordered navigation pages.
- `[[cards]]`: rendered values supplied by a configurable source.
- `[[actions]]`: explicit user-triggered commands with optional confirmation.

A module starts with the same schema version and may have a descriptive name:

```toml
schema_version = 2
name = "workstation"

[[cards]]
# ...one or more complete cards...
```

Duplicate ids are rejected by default. A deliberate personal overlay can set
`replace_existing = true`; that module may replace earlier page/card/action ids
and may own complete `[app]`, `[ui]`, or `[runtime]` sections. Settings are
saved back to the last module that owns the entry, so the default
`config.toml` remains unchanged. See
[the standalone module example](config/config.d/50-custom.example.toml).

Validate the main file, every active module, duplicate/override rules, and
compiled plugin options without opening the UI:

```sh
pulsedeck --check-config
pulsedeck --check-config /path/to/config.toml
```

A minimal custom card is:

```toml
[[cards]]
id = "kernel"
title = "Kernel"
page = "monitor"
renderer = "text"
refresh_interval = 3600

[cards.source]
type = "command"
program = "uname"
args = ["-r"]
timeout_seconds = 5
```

Standard non-plugin cards can also derive named visual states from their current
value. The first matching `[[cards.display.states]]` rule may override the label,
icon, accent, value, progress, and background colors. A `background` array creates
a restrained gradient, while `[cards.display.transition]` smooths state changes
without adding a polling or animation timer. See the card guide for numeric,
text, regex, semantic-level, and source-lifecycle matchers.

When `reload_on_change = true`, changes to the main file and active modules are reloaded
while the app is running. Reopen the app after adding/removing pages or cards so
the complete page hierarchy can be rebuilt.

## Sources and renderers

| Source | Required fields | Use |
| --- | --- | --- |
| `builtin` | `metric` | Efficient native Linux system metrics. |
| `file` | `path` | Read a text/sysfs/procfs file. |
| `command` | `program`, optional `args` | Run a bounded subprocess without a shell. |
| `http` | `url`, optional method/headers/body/parser | Fetch local or remote data. |
| `static_value` | `options.value` | Labels and fixed informational cards. |

Renderers are `value`, `progress`, `status`, `text`, `list`, `composite`, and
`action`. Choose a renderer compatible with the value returned by the source;
built-in metrics already return the appropriate structured value.

## Optional ScrcpyForge integration

Building with `--features scrcpy-forge` automatically creates
`config.d/90-scrcpy-forge.toml` when the page is absent. Builds without that
feature never create the file. Existing page configuration is preserved; the
standalone example is only needed for customization.

The integration is excluded from default builds. Enable the `scrcpy-forge`
feature and copy the standalone
`src/plugins/scrcpy_forge/config.example.toml` module into `config.d/` when
explicit customization is needed. It
connects to a separately installed ScrcpyForge daemon; PulseDeck does not own
ADB or scrcpy processes. Service programs, URLs, and scripts remain configurable.
Its preview and health loops consume the shared runtime mode:

- Foreground normal mode uses the configured preview interval.
- Idle mode keeps lightweight device/script metadata but omits preview frames.
- Hidden pages and background mode stop preview work instead of polling.
- Thermal pressure reduces preview/health frequency, while unchanged frames
  reuse an ETag/content-hash cache.

ScrcpyForge (SF) is a multi-device Android automation project built around ADB
and scrcpy, with device control, previews, and script automation. See the
[ScrcpyForge project](https://github.com/xiangwan-cn/ScrcpyForge) for details.

## Optional Codex/OpenCode/pi PetCard

The `pet-card` feature adds a generic plugin card without adding agent-specific
state or timers to the core. The separately installable Codex hook, OpenCode
plugin, and pi extension under `integrations/pulsedeck-pet` publish fixed
lifecycle states through an atomic runtime file and never read prompt, message,
or tool contents.

Building with `--features pet-card` creates an enabled card in
`config.d/80-pet-card.toml` when no configuration already defines `codex-pet`.
Builds without the feature never create that module. The zero-config fallback
remains available, while custom frame paths stay isolated in the module.

PetCard-only presentation behavior:

- Double-click cycles through normal, four-cell, six-cell, and fullscreen
  presentation; long-press opens a menu for direct selection.

| PetCard presentation | Behavior |
| --- | --- |
| Normal | Keeps PetCard in its original single FlowBox cell. |
| Four cells | Places PetCard across the left two columns and two logical rows; remaining cards fill the columns beside it. |
| Six cells | Places PetCard across the left two columns and three logical rows. |
| Fullscreen | Fills the current page below the toolbar; `Escape` or the restore button returns to the grid. |

- A manual choice is saved outside `config.toml` at
  `${XDG_STATE_HOME:-$HOME/.local/state}/pulsedeck/pet-card-presentation`.
  Any later active state, including `thinking`, `working`, `coding`, or
  `waiting`, restores that last choice automatically.
- After continuous offline time reaches
  `offline_normal_after_seconds` (five minutes by default), PetCard temporarily
  returns to one normal cell. Offline fallback does not overwrite the saved
  choice, so the next active state expands it again.
- Four/six-cell presentation follows the current three- or six-column page
  grid, so changing the toolbar layout immediately reflows the surrounding
  cards.

PetCard is also mode-aware: active tasks use the configured animation rate
(capped at 12 FPS), idle mode uses 1 FPS, hidden/background cards remove their
frame timer, and offline/single-frame states have no animation timer. Agent
tasks retain only their original brightness-protection deadline, including
while waiting for input or confirmation. Completion sound is controlled by the
global runtime setting. See [docs/PET_CARD.md](docs/PET_CARD.md).

![PetCard working in quad presentation](docs/images/pulsedeck-petcard-working.png)

_A complete dark dashboard with PetCard in the working state and four-cell
presentation._

## Project layout

- `src/core`: configuration, runtime/power state, scheduling, caching, and error policy.
- `src/metrics`, `src/sources`, `src/parsers`: data collection and conversion.
- `src/rendering`, `src/ui`: reusable card presentation.
- `src/execution`: bounded subprocess execution for user-triggered actions and sources.
- `src/plugins`: optional external integrations.
- `docs/PET_CARD.md`: optional Codex PetCard build, hook, and asset configuration.
- `docs/RUNTIME_POWER.md`: runtime modes, low-power policies, and validation.
- `config`: portable examples and the card guide.
- `data`: desktop entry and application icon.

## Safety and portability

Commands use explicit argument arrays and enforce timeout/output limits. Actions
run with the current user's privileges unless your local command explicitly
invokes a privilege broker. The committed defaults contain no hostnames,
absolute user paths, device IDs, credentials, or machine-specific tuning. Keep
authenticated HTTP headers in an ignored local config; do not commit them.

## License

MIT
