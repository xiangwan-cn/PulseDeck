# Runtime and power policy

PulseDeck uses one core runtime manager for normal pages and every plugin. The
manager owns foreground state, real user-idle time, bounded interactions,
external-power state, thermal diagnostics, agent task protection, and important
event edges. Plugins consume its snapshot; they do not implement their own
global power decisions.

## Modes and priority

The display mode priority is:

1. background;
2. connected external-power realtime;
3. agent important-event attention;
4. agent new-task brightness protection;
5. stable user idle;
6. foreground normal.

Display and refresh are deliberately separate. Agent protection keeps normal
brightness but ordinary cards may still use throttled refresh after the user
idle threshold. Connected external power can select realtime refresh, while
high-cost command and HTTP cards remain at their original interval unless their
card policy explicitly opts in.

Foreground screen inhibition covers both idle blanking and suspend. It remains
active in normal, idle, and external modes when `keep_screen_on` is enabled.
Background/minimized state releases the inhibitor and suspends nonessential
refresh. Every setting is applied to the live runtime manager after it is saved.

## Idle behavior

Only real input resets idle time: touch/click, scroll, keyboard input, drag,
page changes, manual refresh, dialog responses, and plugin controls.
Hover and pointer motion do not. Automatic card refresh, animation, hook file
changes, plugin polling, network responses, and preview changes are not user
activity. Interaction leases automatically expire after at most five minutes.

After `idle_timeout_seconds`, a separate `idle_stability_seconds` delay prevents
mode oscillation. The optional CPU hint can delay this stability decision for
at most 60 seconds; it never resets user-idle time and is disabled by default.
Any real input restores the UI immediately.

`idle_display = "dim"` places a noninteractive application overlay over the
page. `"minimal"` uses a black OLED-friendly status view containing only time,
runtime/Codex/power state, hides the full dashboard subtree, and uses a discrete
five-minute position shift. Neither mode changes system brightness. Leaving
idle/background or exiting removes the
overlay, so an application error cannot leave a system-wide black screen.

## Refresh policy

The scheduler stores each card's original interval and class. Idle multipliers
are strongest for command/HTTP/network-rate work, moderate for CPU/memory, and
smaller for battery/thermal data. Static cards run once. File cards and network
connection status refresh from events. Fixed wall-clock schedules retain their
slots. Manual refresh bypasses throttling.

Mode transitions replace pending relative deadlines immediately. Nearby
deadlines share a bounded coalescing window to reduce CPU wakeups without
executing scheduled tasks early. Started work completes normally. Background
stops nonessential work.

Optional per-card policy:

```toml
[cards.runtime]
class = "http"
idle_behavior = "throttle"
idle_multiplier = 10.0
external_realtime = false
realtime_multiplier = 0.75
minimum_interval_seconds = 5
```

Persistent command, file, HTTP, and static source objects are reused; HTTP
regexes are precompiled. Memory/Swap share a short-lived procfs snapshot.
Network status uses NetworkManager D-Bus and GIO change notification rather
than repeated `nmcli` subprocesses. Result comparison applies
`minimum_change` to the stable primary value before derived subtitle/tooltip
text. Persistent cache writes are content-deduplicated and rate-limited.
Repeated identical errors are log-limited and a recovery event records return
to health. Initial synchronous card collection has four blocking workers. The
CPU card takes one baseline sample and schedules a single 250 ms follow-up,
then returns to its configured steady interval. A mapped window continues
collecting even when it does not own keyboard focus; unmapping pauses ordinary
work. If startup briefly transitions through suspended mode before the window
maps, cards that have never collected retain an immediate first run, including
event-driven file and network-status cards.

Manual card refresh disables its button until the result returns, while keeping
the previous value visible. Loading, unavailable, and error results use a
renderer-independent static state so list/composite cards cannot retain stale
successful content. Confirmation dialogs hold a bounded interaction lease while
the user reads them; automatically displayed result dialogs only count as
activity after the user closes them.

Standard-card visual-state rules are evaluated only when one of those existing
results is applied. Numeric/text/regex matching, label or icon replacement,
multi-color backgrounds, and CSS color transitions add no scheduler task,
polling interval, or frame timer. Regexes are compiled when display
configuration is applied. A live config reload clears result equivalence once,
requests the card through its existing scheduler, and reapplies the appearance;
steady-state deduplication and `minimum_change` continue normally afterward.
Plugin cards retain their own state and animation policies.

## External power

The power monitor reads charger `online` state plus battery, input-power, and
thermal diagnostics. Runtime policy deliberately uses only the online signal:
when `external_realtime` is enabled, any detected external supply selects
realtime mode even if charging is paused, the battery is full, input telemetry
is zero or unknown, or the thermal verdict is elevated. When
`external_prevents_idle` is enabled, the same online signal prevents foreground
idle. PulseDeck never changes the CPU governor.

On systems with UPower, `PropertiesChanged` signals trigger an immediate sysfs
resample, so plugging or unplugging does not wait for the periodic fallback.
The sysfs directory monitor remains useful for supply add/remove events. While
a supply is online, fallback sampling is capped at five seconds for systems
without usable UPower signals; battery-only idle/background sampling remains
slow to avoid unnecessary wakeups. A confirmed `online=0` edge leaves external
mode immediately without exit hysteresis.

Thermal diagnostics do not change the external-power verdict. They separately
reduce expensive presentation work: warm or hotter states slow ScrcpyForge
preview/health intervals, while hot or throttled states freeze PetCard on its
current frame and remove its animation timer. Idle metadata-only preview and
background stopping still take priority.

## Agent lifecycle

A new task identifier creates one brightness-protection deadline. State
heartbeats and progress do not extend it. After the deadline, normal idle rules
apply. Waiting for user input or confirmation keeps the task active under the
same original protection deadline; it does not reset or extend that deadline.
A terminal or user-attention event contains a separate event identifier:
the first edge plays the configured sound, restores normal display/refresh, and
starts the attention deadline. The same event cannot notify twice. State
monitoring and sound remain available in the background; foreground takeover is
a separate setting and is disabled by default.

PetCard removes frame timers while hidden/background, runs at at most 12 FPS in
normal mode and 1 FPS while idle, caches at most three decoded animation states,
and keeps offline static. ScrcpyForge uses cancellable one-shot mode-aware
preview/health loops, requests metadata only while idle, stops preview in the
background, uses HTTP ETags, and avoids rebuilding unchanged textures.

Battery power telemetry accepts signed `power_now`, `power_avg`, and
`current_now` sysfs values and presents their magnitude; charge/discharge
direction continues to come from the battery status.

## Configuration defaults

The authoritative main-file examples are `config/config.example.toml` and
`config/config.example.json`; a portable module example lives under
`config/config.d/`. The main file is loaded first, followed by top-level TOML
and JSON modules in lexical file-name order. Modules reject duplicate ids unless
they explicitly opt into replacement, and settings are written back to the
owning document rather than flattening the merged configuration. Important defaults are: foreground inhibition
enabled, idle power saving after 60 seconds plus 10 seconds stability, 15%
application visual brightness, balanced refresh saving, external realtime when
a supply reports `online=1`, agent brightness protection for 60
minutes, and 15 seconds of attention after an important event.

Both examples use strict `schema_version = 2`. Runtime and card configuration
does not carry legacy aliases or ignored placeholder fields: a version mismatch,
unknown typed field, or unknown enum value rejects the reload and leaves the
last successfully loaded configuration active. Schema changes therefore update
the repository examples and the active local configuration in the same change.

## Measurement

Build with `--features power-debug` to expose on-demand counters in Settings:
scheduler wakes, card collections, external processes, HTTP requests, image
decodes, animation ticks, GTK updates, disk reads, and disk writes. This feature
adds no periodic sampling and is excluded from default builds.

Validate on the target device with the same brightness and workload:

1. record at least 10 minutes each for foreground normal, foreground idle,
   background, connected external power, and a long-running agent task;
2. compare battery energy/current and `powertop` wakeups, not only average CPU;
3. verify process launches with `execsnoop`/audit tooling and traffic with
   interface counters;
4. compare the internal counters at the start and end of each run;
5. repeat runs because radio, charging, temperature, and battery gauge noise
   can dominate short samples.

The regression suite covers runtime priority, idle entry/recovery, task/event
deduplication, scheduler suspension/event-only sources, and power hysteresis.
Always test the default build, each changed optional plugin feature, their
combination, and `power-debug` before release.
