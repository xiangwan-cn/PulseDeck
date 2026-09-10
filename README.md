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
- Pure facts-to-policy runtime model with separate mapped/active/idle state,
  work and visual levels, screen inhibition, Agent notifications, and optional
  application-only dim/minimal views.
- Event-driven file and network-status cards, coalesced refresh deadlines,
  shared system snapshots, and deduplicated persistent cache writes.
- Bounded subprocess output, HTTP response size, and execution time.
- Optional, separately compiled ScrcpyForge device-control page.
- Optional, separately compiled event-driven Codex/OpenCode/pi PetCard with
  animated lifecycle states, remembered presentation, and completion sound.
- A page-wide toolbar toggle between the configured normal grid and a compact
  six-column grid, with the last choice remembered across launches.

## Runtime and low-power policy

PulseDeck separates runtime facts from a pure policy evaluator. The resulting
snapshot has independent visibility, activity, work, visual, screen-inhibit,
idle-view, power, thermal, and Agent dimensions. A mapped window that loses
keyboard focus is therefore distinct from an unmapped/background window.

Only real input such as click/touch, key press, scroll, drag, page changes,
manual refresh, dialog responses, and plugin controls resets user-idle time.
Automatic refreshes, animations, file events, Agent hooks, and network responses
do not.

| State | B2 default behavior |
| --- | --- |
| Mapped and active | Full ordinary monitor work; auto-classified expensive polling uses a 30-minute floor. |
| Mapped but inactive or locally idle | The same Full daytime monitor policy; focus, idle, and grace are diagnostic/UI facts. |
| Quiet hours | Ordinary periodic local/remote work pauses by the clock; power, battery, thermal, network, Agent signals, and explicit one-shots remain available. |
| Unmapped | Ordinary work and plugin polling are suspended; queued manual/event work waits for mapping. |

`profile = "performance" | "balanced" | "eco"` remains a strict v4 compatibility/diagnostic
field, not an effective Settings control; mapped daytime default work is Full for
all profiles. Auto-classified command/HTTP work is `expensive`, uses a generic
30-minute monitor floor, and pauses under Low/Critical safety caps; classify a
known-cheap source as `normal` or `live` to opt out. Explicit per-card
`inactive_behavior`/`idle_behavior` remains an intentional override.
`screen_inhibit = "never" | "while-active" | "while-mapped"` is independent
from refresh and asks only to inhibit idle blanking, not system suspend.
`idle_view = "none" | "dim" | "minimal"` affects PulseDeck only and never
changes system brightness. Quiet hours are local-clock driven, use `[start,end)`,
and equal hours disable the window; manual/source requests remain allowed.

External power never clears Low/Critical battery stages or promotes mapped
work above Full; `external_boost` is retained only for strict v4 compatibility/diagnostics,
not as an effective Settings control, and is disabled by default. Low/critical battery hysteresis defaults to 20/25% and
10/15%, respectively. Power/thermal signal fallbacks remain bounded
(15–300 seconds for power, 15–60 seconds for thermal). Warm thermal state is
diagnostic only; Hot/Throttled state independently caps work and freezes PetCard.
An observation lease defaults
to 300 seconds, temporarily resumes due monitoring work during quiet hours,
and is renewed only by real mapped input—never by Agent, network, file,
animation, or automatic-refresh events.

Agent state can drive PetCard and deduplicated notifications, but it cannot keep
ordinary cards, dashboard brightness, or screen inhibition at full policy.
See [docs/RUNTIME_POWER.md](docs/RUNTIME_POWER.md) for the full policy, cache,
action-invalidation, and bounded-resume contracts.

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

Configuration uses strict schema v4 and is never migrated during normal
startup. `schema_version = 4` is required at the document root; unknown fields,
obsolete aliases, and unknown enum values reject the configuration instead of
being ignored. Use the explicit `pulsedeck config migrate` command for v3 files.

Start with [config/config.example.toml](config/config.example.toml). A matching
JSON example is available at [config/config.example.json](config/config.example.json).
The current TOML schema is documented with practical card recipes in
[config/CARD_GUIDE.md](config/CARD_GUIDE.md).
PetCard build, hook, animation, sizing, power, and sound behavior is documented
in [docs/PET_CARD.md](docs/PET_CARD.md).
Runtime policy, scheduler behavior, plugin integration, and measurement guidance
are documented in [docs/RUNTIME_POWER.md](docs/RUNTIME_POWER.md).

The top-level sections are:

- `schema_version`: required configuration interface version; currently `4`.
- `[app]`: title, logging, output limits, and config reload.
- `[runtime]`: compatibility diagnostics (`profile`, `external_boost`), mapped/idle
  behavior, screen inhibition, idle view, quiet hours, observation lease,
  power/thermal sampling, battery hysteresis, and Agent notification policy.
- `[ui]`: default page plus normal-grid columns and card dimensions; the live
  normal/compact toolbar choice is stored separately as UI state.
- `[[pages]]`: ordered navigation pages.
- `[[cards]]`: rendered values supplied by a configurable source.
- `[[actions]]`: explicit user-triggered commands with optional confirmation.

A module starts with the same schema version and may have a descriptive name:

```toml
schema_version = 4
name = "workstation"

[[cards]]
# ...one or more complete cards...
```

Duplicate ids are rejected by default. A deliberate personal overlay can set
`replace_existing = true`; that module may replace earlier page/card/action ids
and override individual `[app]`, `[ui]`, or `[runtime]` fields. Omitted fields
continue to inherit the main file. Changed settings are saved back to the last
module that owns the section, so the default
`config.toml` remains unchanged. See
[the standalone module example](config/config.d/50-custom.example.toml).

Validate, format, or generate configuration without opening the UI. Without
`--module`, `add` presents the existing module files plus a create-new choice.
An explicit target may also name an existing file or a new module. Existing
files retain their replacement policy; new personal modules enable
`replace_existing`. Thus a personal override wins over a matching default,
without hard-coding any user-specific destination:

```sh
pulsedeck config check
pulsedeck config check /path/to/config.toml
pulsedeck config migrate # explicit v3 -> v4 migration with .v3.bak backups
pulsedeck config add builtin cpu --id cpu-personal --title CPU --renderer progress --refresh 5s
pulsedeck config add command --id kernel --title Kernel --renderer text --refresh 1h --module 50-workstation.toml -- uname -r
pulsedeck config format # canonicalizes the root and modules; comments are removed
```

A minimal custom card is:

```toml
[[cards]]
id = "kernel"
title = "Kernel"
page = "monitor"
renderer = "text"
refresh = "1h"
source = { command = { run = ["uname", "-r"], timeout = "5s" } }
```

Standard non-plugin cards can also derive named visual states from their current
value. The first matching `[[cards.display.states]]` rule may override the label,
icon, accent, value, progress, and background colors. A `background` array creates
a restrained gradient, while `[cards.display.transition]` smooths state changes
without adding a polling or animation timer. See the card guide for numeric,
text, regex, semantic-level, and source-lifecycle matchers. Standard cards may
also use a static local `[cards.display.background_svg]` plus an overlaid
top-right `[cards.display.logo_svg]`. The logo replaces the refresh icon without
moving the centered title; compact mode keeps it as a non-interactive decoration
while continuing to hide per-card refresh. Neither asset adds a refresh task.

When `reload_on_change = true`, changes to the main file and active modules are reloaded
while the app is running. Reopen the app after adding/removing pages or cards so
the complete page hierarchy can be rebuilt.

## Sources and renderers

| Source syntax | Use |
| --- | --- |
| `{ builtin = "cpu" }` | Efficient native Linux system metrics. |
| `{ file = { path = "/path" } }` | Read a text/sysfs/procfs file. |
| `{ command = { run = ["program", "arg"] } }` | Run a bounded subprocess without a shell. |
| `{ http = { url = "https://…" } }` | Fetch local or remote data. |
| `{ text = "fixed content" }` | Labels and fixed informational cards. |

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
Its preview and health loops map the generic work level locally:

- `Full` uses the configured preview interval.
- `Reduced` slows preview and health checks.
- `Minimal` keeps lightweight device/script metadata but omits preview frames.
- `Suspended` and hidden pages stop preview work instead of polling.
- Unchanged frames continue to reuse an ETag/content-hash cache.

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

PetCard is also policy-aware: any mapped active-Agent state uses the configured
animation rate (capped at 12 FPS) for both active and inactive mapped windows
during daytime or a quiet-hours observation lease. Without a lease, quiet hours
freeze all looping animation while Agent state events continue. Continuous
non-Agent loops are capped at 1 FPS outside quiet hours; finite completion/error
animations may play once. Hot/Throttled freezes the current frame, while Warm
and Unknown do not cap it. Hidden/unmapped cards remove their frame timer, and
Agent animation never promotes ordinary refresh, remote work, brightness, or
screen inhibition. Completion sound is controlled by the global Agent
notification setting. See [docs/PET_CARD.md](docs/PET_CARD.md).

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
- `docs/RUNTIME_POWER.md`: runtime policy, low-power behavior, and validation.
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
