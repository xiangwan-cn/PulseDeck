# Runtime and power policy

PulseDeck models runtime behavior as three layers:

1. **Signals** collect GTK visibility/activity, real user input, bounded interactions,
   power-source state, thermal state, and Agent lifecycle events.
2. **Pure policy** evaluates those facts into one multidimensional snapshot.
3. **Consumers** use only the dimensions they need: the scheduler uses
   `WorkLevel`, the window uses screen-inhibit and idle-view decisions, PetCard
   uses `VisualPolicy`, and ScrcpyForge maps `WorkLevel` to local preview modes.

This avoids a single priority-driven “mode” in which Agent, power, display, and
refresh decisions accidentally promote one another.

## Runtime snapshot

The snapshot contains independent decisions:

- **Visibility:** `Unmapped`, `MappedInactive`, or `MappedActive`.
- **Activity:** `Engaged` or stable `Idle`.
- **Work:** `Full`, `Reduced`, `Minimal`, or `Suspended`.
- **Visuals:** `Full`, `Capped(fps)`, `Frozen`, or `Stopped`.
- **Screen inhibition:** a boolean derived only from `screen_inhibit` and GTK
  visibility/activity; it requests idle blanking inhibition.
- **Suspend inhibition:** a separate boolean derived only from `suspend_inhibit`
  and GTK visibility/activity; it requests an opt-in system suspend inhibitor.
- **Idle view:** none, an application-only dim overlay, or a black minimal view.
- **Periodic refresh pause:** an independent quiet-hours decision; manual refresh remains available.
- **Diagnostics:** power, thermal, Agent phase, attention time, and reason tags.

GTK map/unmap and `is-active` notifications are deliberately separate. A mapped
window that does not own keyboard focus is inactive, not background. PulseDeck
does not implement compositor-specific occlusion detection.

## Profiles and mapped daytime behavior

`[runtime].profile` is retained as a strict v4 compatibility/diagnostic field only; it is
not an effective Settings control.
For the default scheduler policy, every mapped window is a visible monitor:

| State | Ordinary monitor work |
| --- | --- |
| Mapped and active, engaged | Full at each configured base interval |
| Mapped but inactive, before or after grace | Full at each configured base interval |
| Mapped and active, stable idle | Full at each configured base interval |
| Unmapped | Suspended; no new ordinary work starts |

Focus loss, `inactive_grace_seconds`, local idle, `idle_timeout_seconds`, and
`idle_stability_seconds` therefore do not throttle or pause mapped daytime
collection. They remain available for diagnostics and optional idle presentation.
An explicit per-card `inactive_behavior`/`idle_behavior` of `keep`, `throttle`,
or `pause` is an intentional user override and is separate from implicit focus
or idle state.

Only unmapped state, battery safety stages, and genuine thermal pressure may
reduce ordinary work. External power and Agent state never promote ordinary
cards above this policy. Only click/touch, scroll, keyboard input, drag, page
changes, manual refresh, and plugin controls reset the user-idle clock. Automatic
card refresh, animation, hook files, network responses, and preview updates do
not. Observation leases expire after at most five minutes and are renewed only
by real mapped input.

## Screen inhibition and idle view

Screen inhibition is independent from refresh, Agent state, external power, and
idle visuals:

```toml
[runtime]
screen_inhibit = "while-active" # never | while-active | while-mapped
suspend_inhibit = "never"        # never | while-active | while-mapped
```

The default, `while-active`, asks the desktop session to inhibit idle blanking
only while the PulseDeck window is mapped and active. The separate
`suspend_inhibit` setting defaults to `never`; when enabled it asks the desktop
session to inhibit automatic system suspension for the selected mapped/active
state. It is opt-in because it can increase energy use. Unmapping and
application shutdown always release all inhibitors. GTK and the desktop session
may reject or override an inhibitor, so this remains a request rather than an
absolute guarantee.

Idle presentation is also independent:

```toml
idle_view = "none" # none | dim | minimal
idle_visual_brightness_percent = 15
```

`balanced` defaults to `idle_view = "none"`, so idle throttling does not dim the
UI. `dim` adds a noninteractive overlay inside PulseDeck. `minimal` hides the
dashboard subtree and shows a black status/time view. Neither changes system
brightness. Leaving idle removes the overlay immediately.

## Scheduler workload policy

Every ordinary card has a generic workload and per-context behavior:

```toml
[cards.runtime]
workload = "expensive"          # auto | live | normal | expensive | event
inactive_behavior = "inherit"   # inherit | keep | throttle | pause
idle_behavior = "pause"         # inherit | keep | throttle | pause
inactive_interval_seconds = 120  # optional
idle_interval_seconds = 300      # optional
minimum_interval_seconds = 5     # optional
```

`auto` currently infers:

- command and HTTP sources: `expensive`;
- file, static text, battery capacity/temperature, CPU temperature, and built-in network status: `event`;
- built-in network throughput: `live`;
- stateful power and other built-in metrics: `normal`.

Default monitor policy:

- inherited `expensive` work has a 30-minute daytime floor; classify a genuinely cheap command/HTTP source as `normal` or `live` to opt out;
- safety-reduced/minimal work pauses inherited unscheduled `expensive` polling, while fixed schedules and manual requests remain eligible;
- `live` and `normal` work is throttled by bounded workload multipliers under safety caps;
- `event` work runs once initially and then only on source events or bounded source watchdogs;
- a thermally reduced active window throttles expensive work instead of ignoring
  the thermal constraint.

`keep` preserves the configured base interval. `throttle` uses the explicit
context interval when present, otherwise the workload multiplier; it never makes
a card faster than its configured base interval. `pause` disables periodic work.
`minimum_interval_seconds` is always enforced.

The scheduler still preserves these invariants:

- no re-entrant collection of the same card;
- bounded failure backoff;
- page pausing;
- manual/event `run_once` requests, including requests queued while unmapped;
- event-only sources and immediate first collection;
- stale-generation rejection during rescheduling;
- deadline coalescing without running fixed wall-clock schedules early.

Unmapped `Suspended` is a hard stop. Work already started may finish, but new
ordinary work does not start. A manual/event request made while suspended is
queued and runs when work resumes. Fixed schedule deadlines are preserved across
policy changes.

Quiet hours are clock-driven, not idle-driven. During the half-open local-time
window, ordinary periodic local/remote work is paused even when the window is
focused. Power/battery/thermal and NetworkManager signal paths, Agent state-file
events, and explicit manual/source requests remain available. A real mapped
input observation lease temporarily resumes due monitoring work for the configured
lease duration. Automatic events never create or renew that lease. On leaving quiet
hours, due work is resumed through
a bounded queue rather than all at once:

```toml
quiet_hours_enabled = true
quiet_hours_start_hour = 0
quiet_hours_end_hour = 8
```

Hours use local time and the half-open range `[start, end)`. Overnight ranges
such as `22` to `6` are supported. Equal start/end hours disable the window.

### Observation lease

`observation_lease_seconds` defaults to 300 and is clamped to the same five-minute
hard maximum. Click/touch, key, scroll, drag, page-switch, manual-refresh,
dialog, and plugin-control input starts or renews a lease only while the window
is mapped. Agent events, file/network events, automatic refresh, responses,
animations, and elapsed time never renew it. Dialog/plugin owners may hold a
shorter interaction lease; dropping that owner releases its part immediately.
During a quiet-hours observation lease, due local and remote work resumes through
the normal bounded scheduler; only real input can extend the observation window.

### Battery stage and hysteresis

Battery safety is separate from the display colour threshold. The default
hysteresis pairs are:

| Transition | Capacity |
| --- | ---: |
| Normal → Low | ≤ 20% |
| Low → Normal | ≥ 25% |
| Low → Critical | ≤ 10% |
| Critical → Low | ≥ 15% |

The stage persists between each pair of thresholds; external-power insertion,
quiet boundaries, Agent state, and missing/invalid capacity do not clear it.
`Low` caps ordinary work at `Reduced`, while `Critical` caps it at `Minimal`;
signal samplers and explicit user one-shots remain separate. An unknown first
sample never grants an external-power boost.

### Event, cache, and resume guarantees

File, network, power, and Agent edges are coalesced per source/task: an idle
card gets at most one queued collection, and a burst during a running
collection gets at most one dirty follow-up for the newest source revision.
Source identity is descriptor-based rather than card-ID-based. A stale worker
or old configuration epoch is rejected before rendering or rescheduling.

Only a successful non-cached source result replaces the last-good cache.
`Loading`, `Unavailable`, and `Error` retain the previous successful value;
when rendered as a fallback it is marked stale. Unchanged semantic values do
not repaint GTK or rewrite persistent cache age. Failed collections still use
bounded exponential backoff, while every event source keeps a bounded watchdog.
Fixed counter/window samplers (CPU, network throughput, and power averages)
retain their required spacing and state.

A quiet-hours exit or successful configuration reload preserves fixed schedule
anchors and spreads remote/expensive starts by at least a 500 ms slot. Signal
and explicit manual work take priority, and no card has more than one queued
instance.

Action completion is generic: every enabled card whose `click_action` matches
the completed action is invalidated and queued once on both success and failure,
including hidden controls; unrelated cards are untouched.

## External power and thermal sampling

Power-source and thermal sampling are logically separate and have independent
fallback timers.

- UPower `PropertiesChanged` and sysfs supply-directory events trigger immediate
  power resampling after a short debounce.
- `power_sample_seconds` is a bounded 15–300 second fallback cadence in every
  mapped/unmapped and engaged/idle state; battery capacity/status, temperature,
  and power facts therefore remain fresh overnight.
- `thermal_sample_seconds` defaults to 30 seconds and is clamped to 15–60 seconds.
- NetworkManager and file event sources retain their own bounded watchdogs;
  signal loss cannot make an event-driven value stale forever.

The power verdict reports battery, external, or unknown. Detailed charging,
energy, and power telemetry remains the responsibility of metric sources rather
than the runtime policy signal. Thermal reports normal, warm, hot, throttled, or
unknown and never changes the power-source verdict.

`external_boost` is retained only as a strict v4 compatibility/diagnostic field,
not an effective Settings control. Under the B2 mapped-daytime rule, mapped
ordinary work is already Full, unmapped work is Suspended, and battery or
thermal stages are safety caps, so external power never promotes ordinary work
or clears a safety stage. It does not reset idle time, alter quiet hours, change
idle visuals, inhibit the screen, or make Agent activity globally important.

Warm conditions are diagnostic only and do not reduce work or PetCard FPS.
Battery temperature reaches warm/hot at 42/48°C; CPU/SoC reaches warm/hot at
80/90°C. Hot freezes expensive visuals and lowers work to at most `Reduced`;
kernel-reported thermal pressure lowers work to `Minimal`. PulseDeck never
changes the CPU governor and does not create cgroups or systemd scopes.

## Agent lifecycle

Agent lifecycle is diagnostic and notification state, not a global runtime
priority. An active Agent may let PetCard use its full configured animation rate,
but it cannot:

- keep ordinary cards at full refresh;
- reset the user-idle clock;
- brighten the whole dashboard;
- enable screen inhibition;
- override thermal or unmapped policy.

Distinct completion, failure, cancellation, waiting-input,
confirmation-required, and abort event identifiers are deduplicated. An event
may play one configured sound and starts `agent_attention_seconds`. Optional
`bring_to_foreground_on_attention` may present the window, but the Agent event
still does not promote scheduler or screen policy.

## Plugin adaptation

### PetCard

PetCard consumes generic `VisualPolicy` plus its own lifecycle state:

- any mapped active-Agent state uses configured FPS, capped at 12, when the
  dashboard is inactive or locally idle, and during a quiet-hours observation
  lease; quiet hours without a lease freeze looping animation but retain events;
- continuously looping non-Agent states such as `ready` are capped at 1 FPS
  outside quiet hours and frozen during quiet hours;
- finite completion/error animations may play once as event acknowledgement;
- Hot or Throttled freezes the current frame; Warm and Unknown do not cap it;
- hidden or unmapped cards remove their frame timer;
- offline and single-frame states have no frame timer.

The Agent exception is local to PetCard. It does not promote ordinary refresh,
remote preview/health work, screen inhibition, or dashboard brightness.

PetCard keeps only a three-state decoded-frame cache.

### ScrcpyForge

ScrcpyForge defines its preview mapping locally rather than exposing a
plugin-specific enum in core:

- `Full`: configured preview and health intervals;
- `Reduced`: slower preview and health intervals;
- `Minimal`: metadata only, without preview-frame requests;
- `Suspended`, or idle quiet hours: no preview or health polling.

Its loops race timers against runtime and widget visibility notifications so
transitions take effect immediately. Existing ETag/content-hash reuse remains.

## Strict schema v4 and migration

Normal startup never rewrites an existing configuration. Schema v4 rejects an
incorrect version, unknown field, obsolete alias, or unknown enum value.

Migrate v3 explicitly:

```sh
pulsedeck config migrate
pulsedeck config migrate /path/to/config.toml
pulsedeck config check
```

Migration includes top-level TOML/JSON files under the adjacent `config.d/`,
validates converted documents, prepares all outputs before replacement, and
restores originals if merged v4 validation fails. On success each source file
has a `.v3.bak` backup. Migration canonicalizes formatting and does not retain
comments.

Important semantic mappings:

- disabled v3 idle saving is decoded into compatibility field `profile = "performance"`;
- v3 aggressive saving is decoded into compatibility field `profile = "eco"`; mild/balanced become
  `balanced`; these fields do not change B2 policy;
- `keep_screen_on = true` becomes `screen_inhibit = "while-mapped"` to preserve
  the old mapped-window behavior;
- `external_realtime` is decoded into compatibility field `external_boost`; the removed
  `external_prevents_idle` behavior has no v4 equivalent and the field has no B2 effect;
- v3 `codex_attention_seconds` and `codex_completion_sound` become generic
  Agent notification fields;
- v3 external hysteresis knobs, Agent brightness protection, CPU activity hints,
  and multiplier fields are removed;
- card classes map to `live`, `normal`, `expensive`, or `event` workloads.

Migration changes only fields present in each document, so a small overlay does
not accidentally gain unrelated profile, screen, or idle-view overrides.

## Configuration defaults

```toml
[runtime]
# profile and external_boost are compatibility diagnostics and are omitted here.
inactive_grace_seconds = 15
screen_inhibit = "while-active"
suspend_inhibit = "never"
idle_timeout_seconds = 60
idle_stability_seconds = 10
idle_view = "none"
idle_visual_brightness_percent = 15
quiet_hours_enabled = false
quiet_hours_start_hour = 0
quiet_hours_end_hour = 8
power_sample_seconds = 30
thermal_sample_seconds = 30
agent_attention_seconds = 15
agent_completion_sound = true
bring_to_foreground_on_attention = false
observation_lease_seconds = 300
battery_low_enter_percent = 20
battery_low_exit_percent = 25
battery_critical_enter_percent = 10
battery_critical_exit_percent = 15
```

## Measurement and validation

Build with `--features power-debug` to expose on-demand counters in Settings:
scheduler wakes, card collections, external processes, HTTP requests, image
decodes, animation ticks, GTK updates, disk reads, and disk writes. This feature
adds no periodic sampler.

For device measurements, keep brightness and workload constant and compare at
least these cases over repeated runs:

1. mapped active/engaged;
2. mapped inactive before and after grace;
3. mapped active/stable idle;
4. unmapped;
5. external power with boost disabled and enabled;
6. warm/hot thermal policy;
7. active and completed Agent lifecycle states.

Use battery energy/current and wakeup measurements such as `powertop`, not only
average CPU. Correlate them with PulseDeck’s internal counters and process/network
tracing where available.
