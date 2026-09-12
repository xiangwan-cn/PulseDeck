use super::config::{IdleViewMode, RuntimeConfig, ScreenInhibitMode, SuspendInhibitMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerVerdict {
    Battery,
    External,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStage {
    /// A valid capacity has not been observed yet. This stage never grants an
    /// external-power boost; RuntimeManager retains it until a valid sample.
    Unknown,
    Normal,
    Low,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalVerdict {
    Normal,
    Warm,
    Hot,
    Throttled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentPhase {
    None,
    Active { task_id: String },
    Attention { task_id: String, event_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Unmapped,
    MappedInactive,
    MappedActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Engaged,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkLevel {
    Suspended,
    Minimal,
    Reduced,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualPolicy {
    Full,
    #[cfg(feature = "pet-card")]
    Capped(u32),
    Frozen,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleViewDecision {
    None,
    Dim(u8),
    Minimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFacts {
    pub mapped: bool,
    pub active: bool,
    pub inactive_grace_elapsed: bool,
    pub user_idle: bool,
    pub interaction_active: bool,
    pub quiet_hours: bool,
    pub power: PowerVerdict,
    pub thermal: ThermalVerdict,
    pub battery_stage: BatteryStage,
    pub agent_phase: AgentPhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub visibility: Visibility,
    pub activity: Activity,
    pub work_level: WorkLevel,
    pub visual_policy: VisualPolicy,
    pub inhibit_screen: bool,
    pub inhibit_suspend: bool,
    pub idle_view: IdleViewDecision,
    pub periodic_refresh_paused: bool,
    pub reasons: Vec<String>,
    pub agent_phase: AgentPhase,
    pub interaction_active: bool,
    pub power_verdict: PowerVerdict,
    pub thermal_verdict: ThermalVerdict,
    pub battery_stage: BatteryStage,
    pub observation_lease_active: bool,
    pub attention_remaining_seconds: u64,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            visibility: Visibility::MappedActive,
            activity: Activity::Engaged,
            work_level: WorkLevel::Full,
            visual_policy: VisualPolicy::Full,
            inhibit_screen: true,
            inhibit_suspend: false,
            idle_view: IdleViewDecision::None,
            periodic_refresh_paused: false,
            reasons: vec!["application-start".into()],
            agent_phase: AgentPhase::None,
            interaction_active: false,
            power_verdict: PowerVerdict::Unknown,
            thermal_verdict: ThermalVerdict::Unknown,
            battery_stage: BatteryStage::Unknown,
            observation_lease_active: false,
            attention_remaining_seconds: 0,
        }
    }
}

pub fn quiet_hours_active(enabled: bool, start_hour: u8, end_hour: u8, hour: u8) -> bool {
    if !enabled || start_hour > 23 || end_hour > 23 || hour > 23 || start_hour == end_hour {
        return false;
    }
    if start_hour < end_hour {
        hour >= start_hour && hour < end_hour
    } else {
        hour >= start_hour || hour < end_hour
    }
}

pub fn evaluate(facts: &RuntimeFacts, config: &RuntimeConfig) -> RuntimeSnapshot {
    let visibility = if !facts.mapped {
        Visibility::Unmapped
    } else if facts.active {
        Visibility::MappedActive
    } else {
        Visibility::MappedInactive
    };
    let activity = if facts.user_idle && !facts.interaction_active {
        Activity::Idle
    } else {
        Activity::Engaged
    };

    let mut reasons = vec![match visibility {
        Visibility::Unmapped => "window-unmapped",
        Visibility::MappedInactive => "window-mapped-inactive",
        Visibility::MappedActive => "window-mapped-active",
    }
    .into()];
    if activity == Activity::Idle {
        // Idle is retained as a diagnostic/UI fact. It is never evidence that
        // the user left the device and therefore never changes daytime work.
        reasons.push("user-idle-diagnostic".into());
    }
    if facts.inactive_grace_elapsed && visibility == Visibility::MappedInactive {
        reasons.push("inactive-grace-elapsed-diagnostic".into());
    }

    // Mapping is the ordinary work permit. Focus, local interaction-idle, and
    // profile do not throttle a visible daytime monitor; explicit card
    // behavior is applied by the scheduler separately.
    let mut work_level = if visibility == Visibility::Unmapped {
        WorkLevel::Suspended
    } else {
        WorkLevel::Full
    };

    match facts.battery_stage {
        BatteryStage::Low => {
            work_level = work_level.min(WorkLevel::Reduced);
            reasons.push("battery-low".into());
        }
        BatteryStage::Critical => {
            work_level = work_level.min(WorkLevel::Minimal);
            reasons.push("battery-critical".into());
        }
        BatteryStage::Unknown => reasons.push("battery-stage-unknown".into()),
        BatteryStage::Normal => {}
    }

    // Mapped daytime work is already Full, while unmapped is a hard stop and
    // battery/thermal caps are safety limits. Consequently external power
    // cannot promote ordinary work in any B2 state; retain the setting as a
    // compatibility/diagnostic switch without letting it bypass a cap.
    if config.external_boost && facts.power == PowerVerdict::External {
        reasons.push("external-power-no-daytime-boost".into());
    }

    match facts.thermal {
        ThermalVerdict::Warm => reasons.push("thermal-warm-diagnostic".into()),
        ThermalVerdict::Hot => {
            work_level = work_level.min(WorkLevel::Reduced);
            reasons.push("thermal-hot".into());
        }
        ThermalVerdict::Throttled => {
            work_level = work_level.min(WorkLevel::Minimal);
            reasons.push("thermal-pressure".into());
        }
        ThermalVerdict::Normal | ThermalVerdict::Unknown => {}
    }

    let mut visual_policy = if visibility == Visibility::Unmapped {
        VisualPolicy::Stopped
    } else {
        // PetCard applies its own active-Agent and unattended-loop policy to
        // this mapped-visible baseline. Thermal pressure is the only generic
        // freeze reason.
        VisualPolicy::Full
    };
    if visibility != Visibility::Unmapped
        && matches!(
            facts.thermal,
            ThermalVerdict::Hot | ThermalVerdict::Throttled
        )
    {
        visual_policy = VisualPolicy::Frozen;
    }

    let inhibit_screen = match config.screen_inhibit {
        ScreenInhibitMode::Never => false,
        ScreenInhibitMode::WhileActive => visibility == Visibility::MappedActive,
        ScreenInhibitMode::WhileMapped => visibility != Visibility::Unmapped,
    };
    let inhibit_suspend = match config.suspend_inhibit {
        SuspendInhibitMode::Never => false,
        SuspendInhibitMode::WhileActive => visibility == Visibility::MappedActive,
        SuspendInhibitMode::WhileMapped => visibility != Visibility::Unmapped,
    };

    // Idle presentation is intentionally independent from freshness/work.
    let idle_view = if activity != Activity::Idle || visibility == Visibility::Unmapped {
        IdleViewDecision::None
    } else {
        match config.idle_view {
            IdleViewMode::None => IdleViewDecision::None,
            IdleViewMode::Dim => {
                IdleViewDecision::Dim(config.idle_visual_brightness_percent.min(100))
            }
            IdleViewMode::Minimal => IdleViewDecision::Minimal,
        }
    };

    // Quiet hours are clock-driven. Real mapped input opens a bounded
    // observation window; while it is active, due monitoring work may resume.
    // Source/manual requests remain available regardless of the lease.
    let periodic_refresh_paused = facts.quiet_hours && !facts.interaction_active;
    if facts.quiet_hours {
        reasons.push(if facts.interaction_active {
            "quiet-hours-observation-lease".into()
        } else {
            "quiet-hours".into()
        });
    }
    if !matches!(facts.agent_phase, AgentPhase::None) {
        reasons.push("agent-state-observed".into());
    }

    RuntimeSnapshot {
        visibility,
        activity,
        work_level,
        visual_policy,
        inhibit_screen,
        inhibit_suspend,
        idle_view,
        periodic_refresh_paused,
        reasons,
        agent_phase: facts.agent_phase.clone(),
        interaction_active: facts.interaction_active,
        power_verdict: facts.power,
        thermal_verdict: facts.thermal,
        battery_stage: facts.battery_stage,
        observation_lease_active: facts.interaction_active,
        attention_remaining_seconds: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::RuntimeProfile;

    fn facts() -> RuntimeFacts {
        RuntimeFacts {
            mapped: true,
            active: true,
            inactive_grace_elapsed: false,
            user_idle: false,
            interaction_active: false,
            quiet_hours: false,
            power: PowerVerdict::Battery,
            thermal: ThermalVerdict::Normal,
            battery_stage: BatteryStage::Normal,
            agent_phase: AgentPhase::None,
        }
    }

    #[test]
    fn mapped_daytime_work_ignores_focus_idle_grace_and_profile() {
        let mut config = RuntimeConfig::default();
        for profile in [
            RuntimeProfile::Performance,
            RuntimeProfile::Balanced,
            RuntimeProfile::Eco,
        ] {
            config.profile = profile;
            for (active, idle, grace) in [
                (true, false, false),
                (false, false, false),
                (false, true, true),
            ] {
                let mut input = facts();
                input.active = active;
                input.user_idle = idle;
                input.inactive_grace_elapsed = grace;
                let snapshot = evaluate(&input, &config);
                assert_eq!(snapshot.work_level, WorkLevel::Full);
                assert_eq!(snapshot.visual_policy, VisualPolicy::Full);
            }
        }
        let mut unmapped = facts();
        unmapped.mapped = false;
        assert_eq!(
            evaluate(&unmapped, &config).work_level,
            WorkLevel::Suspended
        );
    }

    #[test]
    fn quiet_hours_are_clock_driven_and_real_input_opens_observation_window() {
        assert!(quiet_hours_active(true, 0, 8, 0));
        assert!(quiet_hours_active(true, 0, 8, 7));
        assert!(!quiet_hours_active(true, 0, 8, 8));
        assert!(quiet_hours_active(true, 22, 6, 23));
        assert!(quiet_hours_active(true, 22, 6, 5));
        assert!(!quiet_hours_active(true, 22, 6, 12));
        assert!(!quiet_hours_active(true, 8, 8, 8));

        let mut input = facts();
        input.quiet_hours = true;
        assert!(evaluate(&input, &RuntimeConfig::default()).periodic_refresh_paused);
        input.interaction_active = true;
        let leased = evaluate(&input, &RuntimeConfig::default());
        assert!(!leased.periodic_refresh_paused);
        assert!(leased.observation_lease_active);
    }

    #[test]
    fn battery_and_thermal_caps_have_expected_precedence() {
        let config = RuntimeConfig::default();
        let mut input = facts();
        input.battery_stage = BatteryStage::Low;
        assert_eq!(evaluate(&input, &config).work_level, WorkLevel::Reduced);
        input.battery_stage = BatteryStage::Critical;
        assert_eq!(evaluate(&input, &config).work_level, WorkLevel::Minimal);
        input.thermal = ThermalVerdict::Hot;
        assert_eq!(evaluate(&input, &config).work_level, WorkLevel::Minimal);
        assert_eq!(
            evaluate(&input, &config).visual_policy,
            VisualPolicy::Frozen
        );
        input.thermal = ThermalVerdict::Warm;
        assert_eq!(evaluate(&input, &config).visual_policy, VisualPolicy::Full);
    }

    #[test]
    fn agent_state_does_not_promote_ordinary_work() {
        let config = RuntimeConfig::default();
        let mut input = facts();
        input.active = false;
        input.agent_phase = AgentPhase::Attention {
            task_id: "task".into(),
            event_id: "event".into(),
        };
        let with_agent = evaluate(&input, &config);
        input.agent_phase = AgentPhase::None;
        let without_agent = evaluate(&input, &config);
        assert_eq!(with_agent.work_level, without_agent.work_level);
        assert_eq!(with_agent.visual_policy, without_agent.visual_policy);
        assert_eq!(with_agent.inhibit_screen, without_agent.inhibit_screen);
    }

    #[test]
    fn external_power_never_overrides_safety_stage() {
        let config = RuntimeConfig {
            external_boost: true,
            ..RuntimeConfig::default()
        };
        let mut input = facts();
        input.power = PowerVerdict::External;
        input.battery_stage = BatteryStage::Low;
        assert_eq!(evaluate(&input, &config).work_level, WorkLevel::Reduced);
        input.battery_stage = BatteryStage::Critical;
        assert_eq!(evaluate(&input, &config).work_level, WorkLevel::Minimal);
        input.battery_stage = BatteryStage::Unknown;
        assert_eq!(evaluate(&input, &config).work_level, WorkLevel::Full);
    }

    #[test]
    fn screen_inhibit_modes_are_independent() {
        let mut config = RuntimeConfig::default();
        let mut input = facts();
        config.screen_inhibit = ScreenInhibitMode::Never;
        assert!(!evaluate(&input, &config).inhibit_screen);
        config.screen_inhibit = ScreenInhibitMode::WhileMapped;
        input.active = false;
        assert!(evaluate(&input, &config).inhibit_screen);
        config.screen_inhibit = ScreenInhibitMode::WhileActive;
        assert!(!evaluate(&input, &config).inhibit_screen);
    }

    #[test]
    fn suspend_inhibit_modes_are_independent_from_screen_inhibit() {
        let mut config = RuntimeConfig {
            screen_inhibit: ScreenInhibitMode::Never,
            suspend_inhibit: SuspendInhibitMode::WhileActive,
            ..RuntimeConfig::default()
        };
        let mut input = facts();
        assert!(!evaluate(&input, &config).inhibit_screen);
        assert!(evaluate(&input, &config).inhibit_suspend);

        input.active = false;
        assert!(!evaluate(&input, &config).inhibit_suspend);
        config.suspend_inhibit = SuspendInhibitMode::WhileMapped;
        assert!(evaluate(&input, &config).inhibit_suspend);

        input.mapped = false;
        assert!(!evaluate(&input, &config).inhibit_suspend);
    }
}
