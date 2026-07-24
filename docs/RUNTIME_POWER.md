# Runtime and power policy

PulseDeck uses one core runtime manager for normal pages and every plugin. The
manager owns foreground state, real user-idle time, bounded interactions,
validated external power, thermal state, Codex task protection, and important
event edges. Plugins consume its snapshot; they do not implement their own
global power decisions.

## Modes and priority

The display mode priority is:

1. background;
2. stable external-power realtime;
3. Codex important-event attention;
4. Codex new-task brightness protection;
5. stable user idle;
6. foreground normal.

Display and refresh are deliberately separate. Codex protection keeps normal
brightness but ordinary cards may still use throttled refresh after the user
idle threshold. Stable external power can select realtime refresh, while
high-cost command and HTTP cards remain at their original interval unless their
card policy explicitly opts in.

Foreground screen inhibition covers both idle blanking and suspend. It remains
active in normal, idle, and external modes when `keep_screen_on` is enabled.
Background/minimized state releases the inhibitor and suspends nonessential
refresh. Every setting is applied to the live runtime manager after it is saved.

## Idle behavior

Only real input resets idle time: touch/click, scroll, keyboard input, drag,
page changes, manual refresh, dialogs, plugin controls, and remote control.
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
to health.

## External power

The power monitor combines charger online state, battery status and trend,
reported input power where available, battery/SoC temperature, and thermal
pressure. Entry requires consecutive trustworthy samples; unplugging and
negative/thermal samples downshift faster. A cable with continuing discharge,
unknown/insufficient margin, or thermal pressure does not select realtime mode.
PulseDeck never changes the CPU governor.

## Codex lifecycle

A new task identifier creates one brightness-protection deadline. State
heartbeats and progress do not extend it. After the deadline, normal idle rules
apply. A terminal or user-attention event contains a separate event identifier:
the first edge plays the configured sound, restores normal display/refresh, and
starts the attention deadline. The same event cannot notify twice. State
monitoring and sound remain available in the background; foreground takeover is
a separate setting and is disabled by default.

PetCard removes frame timers while hidden/background, runs at at most 12 FPS in
normal mode and 1 FPS while idle, caches at most three decoded animation states,
and keeps offline static. ScrcpyForge uses cancellable one-shot mode-aware
preview/health loops, requests metadata only while idle, stops preview in the
background, uses HTTP ETags, and avoids rebuilding unchanged textures.

## Configuration defaults

The authoritative examples are `config/config.example.toml` and
`config/config.example.json`. Important defaults are: foreground inhibition
enabled, idle power saving after 60 seconds plus 10 seconds stability, 15%
application visual brightness, balanced refresh saving, external realtime with
three stable entry samples/two exit samples, Codex brightness protection for 60
minutes, and 15 seconds of attention after an important event.

## Measurement

Build with `--features power-debug` to expose on-demand counters in Settings:
scheduler wakes, card collections, external processes, HTTP requests, image
decodes, animation ticks, GTK updates, disk reads, and disk writes. This feature
adds no periodic sampling and is excluded from default builds.

Validate on the target device with the same brightness and workload:

1. record at least 10 minutes each for foreground normal, foreground idle,
   background, stable external power, and long-running Codex;
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
