# PulseDeck

[简体中文](README_CN.md) | English

PulseDeck is a lightweight, configuration-driven GTK4/Libadwaita dashboard for
Linux phones, tablets, and desktops. Pages, metric cards, refresh schedules,
parsers, and actions are described in TOML or JSON, so most dashboard changes
do not require recompiling the application.

## Features

- Native CPU, memory, battery, power, network, uptime, filesystem, process,
  load, swap, temperature, and network-throughput metrics.
- Built-in, file, command, HTTP, and static-value card sources.
- Value, progress, status, text, list, composite, and action renderers.
- Consistent human-readable primary values: compact percentages, natural unit
  spacing, and status-first network/power summaries.
- Fixed intervals or schedules such as `daily@08:00,20:00`, with per-slot cache.
- Global and per-card responsive sizing for mobile and desktop layouts.
- Page lifecycle awareness: hidden pages stop polling.
- Unified foreground, idle-power, external-power, background, and Codex
  attention state, with live settings and an application-only dim/minimal mode.
- Event-driven file and network-status cards, coalesced refresh deadlines,
  shared system snapshots, and deduplicated persistent cache writes.
- Bounded subprocess output, HTTP response size, and execution time.
- Optional, separately compiled ScrcpyForge device-control page.
- Optional, separately compiled event-driven Codex PetCard with animated
  status, remembered sizing, and a completion sound.
- A remembered toolbar toggle between the normal grid and a compact six-column
  layout.

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

Start with [config/config.example.toml](config/config.example.toml). A matching
JSON example is available at [config/config.example.json](config/config.example.json).
The current TOML schema is documented with practical card recipes in
[config/CARD_GUIDE.md](config/CARD_GUIDE.md).
PetCard build, hook, animation, sizing, power, and sound behavior is documented
in [docs/PET_CARD.md](docs/PET_CARD.md).
Runtime modes, scheduler policy, plugin integration, and measurement guidance
are documented in [docs/RUNTIME_POWER.md](docs/RUNTIME_POWER.md).

The top-level sections are:

- `[app]`: title, logging, output limits, and config reload.
- `[runtime]`: foreground inhibition, idle display, refresh policy, external
  power validation, and Codex protection/notification policy.
- `[ui]`: default page, columns, card dimensions, and compact layout.
- `[[pages]]`: ordered navigation pages.
- `[[cards]]`: rendered values supplied by a configurable source.
- `[[actions]]`: explicit user-triggered commands with optional confirmation.

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

When `reload_on_change = true`, value-level configuration changes are reloaded
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

The integration is excluded from default builds. Enable the `scrcpy-forge`
feature and append the generic configuration from
`src/plugins/scrcpy_forge/config.example.toml` to your local TOML file. It
connects to a separately installed ScrcpyForge daemon; PulseDeck does not own
ADB or scrcpy processes. Service programs, URLs, and scripts remain configurable.
Its preview and health loops consume the shared runtime mode: hidden/background
pages stop preview work, idle mode requests metadata only, and unchanged images
reuse an ETag/hash cache.

## Optional Codex PetCard

The `pet-card` feature adds a generic plugin card without adding Codex-specific
state or timers to the core. The separately installable hook under
`integrations/pulsedeck-pet` publishes fixed lifecycle states through an atomic
runtime file and never reads prompt or tool contents. Offline mode is static,
animations stop while unmapped or in the background, and the global runtime
setting controls completion sound. See
[docs/PET_CARD.md](docs/PET_CARD.md).

## Project layout

- `src/core`: configuration, runtime/power state, scheduling, caching, and registries.
- `src/metrics`, `src/sources`, `src/parsers`: data collection and conversion.
- `src/rendering`, `src/ui`: reusable card presentation.
- `src/actions`: bounded user-triggered actions.
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
