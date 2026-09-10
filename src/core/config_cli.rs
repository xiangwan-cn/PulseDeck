use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::core::config::{
    config_path, parse_duration, AppConfig, CardConfig, CardRuntimeConfig, CommandSourceConfig,
    ConfigFragment, ConfigManager, ConfigModuleInfo, FileSourceConfig, HttpSourceConfig,
    SourceConfig, CONFIG_SCHEMA_VERSION,
};
use crate::model::card_model::RendererKind;

const USAGE: &str = "\
PulseDeck configuration tools

  pulsedeck config check [CONFIG_FILE]
  pulsedeck config format [CONFIG_FILE]
  pulsedeck config migrate [CONFIG_FILE]
  pulsedeck config add builtin METRIC --id ID [OPTIONS]
  pulsedeck config add command --id ID [OPTIONS] -- PROGRAM [ARG ...]
  pulsedeck config add file PATH --id ID [OPTIONS]
  pulsedeck config add http URL --id ID [OPTIONS]
  pulsedeck config add text VALUE --id ID [OPTIONS]

Common add options:
  --title TEXT          defaults to the card id
  --page ID             defaults to monitor
  --module NAME_OR_FILE choose an existing module or create a new one
  --renderer KIND       text, value, progress, status, list, or composite
  --refresh DURATION    for example 5s, 2m, 1h, or 1d
  --order NUMBER
  --icon NAME
  --description TEXT
  --disabled
  --config PATH         defaults to ~/.config/pulsedeck/config.toml
";

pub fn run_if_requested(arguments: &[String]) -> Option<glib::ExitCode> {
    if arguments.first().map(String::as_str) != Some("config") {
        return None;
    }
    let result = run(&arguments[1..]);
    Some(match result {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            glib::ExitCode::FAILURE
        }
    })
}

fn run(arguments: &[String]) -> Result<(), String> {
    match arguments.first().map(String::as_str) {
        Some("check") => check(&arguments[1..]),
        Some("format") => format(&arguments[1..]),
        Some("migrate") => migrate(&arguments[1..]),
        Some("add") => add(&arguments[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(command) => Err(format!("unknown config command: {command}\n\n{USAGE}")),
    }
}

fn check(arguments: &[String]) -> Result<(), String> {
    let path = single_optional_path(arguments, "check")?;
    let mut manager = ConfigManager::new(path.clone());
    manager
        .load()
        .and_then(|()| crate::plugins::validate_config(manager.config()))
        .map_err(|error| format!("configuration invalid at {}: {error}", path.display()))?;
    println!(
        "configuration valid: {} modules, {} pages, {} cards, {} actions",
        manager.loaded_module_count(),
        manager.config().pages.len(),
        manager.config().cards.len(),
        manager.config().actions.len()
    );
    Ok(())
}

fn format(arguments: &[String]) -> Result<(), String> {
    let path = single_optional_path(arguments, "format")?;
    let mut manager = ConfigManager::new(path.clone());
    manager
        .load()
        .map_err(|error| format!("cannot format {}: {error}", path.display()))?;
    crate::plugins::validate_config(manager.config())
        .map_err(|error| format!("cannot format invalid configuration: {error}"))?;
    manager
        .format_documents()
        .map_err(|error| format!("cannot format {}: {error}", path.display()))?;
    println!(
        "formatted {} and {} modules (comments are not retained)",
        path.display(),
        manager.loaded_module_count()
    );
    Ok(())
}

fn migrate(arguments: &[String]) -> Result<(), String> {
    let path = single_optional_path(arguments, "migrate")?;
    let mut documents = vec![path.clone()];
    let modules = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("config.d");
    match std::fs::read_dir(&modules) {
        Ok(entries) => {
            let mut module_paths = Vec::new();
            for entry in entries {
                let path = entry
                    .map_err(|error| format!("cannot read {}: {error}", modules.display()))?
                    .path();
                if path.is_file()
                    && matches!(
                        path.extension().and_then(|value| value.to_str()),
                        Some("toml" | "json")
                    )
                {
                    module_paths.push(path);
                }
            }
            module_paths.sort();
            documents.extend(module_paths);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot read {}: {error}", modules.display())),
    }

    let mut converted = Vec::new();
    for (index, document) in documents.into_iter().enumerate() {
        let text = std::fs::read_to_string(&document)
            .map_err(|error| format!("cannot read {}: {error}", document.display()))?;
        match document_schema(&document, &text)? {
            3 => {
                let output = migrate_document(&document, &text)?;
                validate_migrated_document(&document, &output, index == 0)?;
                converted.push((document, output));
            }
            4 => validate_migrated_document(&document, &text, index == 0)?,
            version => {
                return Err(format!(
                    "{} uses unsupported schema v{version}; expected v3 or v4",
                    document.display()
                ));
            }
        }
    }
    if converted.is_empty() {
        println!("configuration is already schema v4: {}", path.display());
        return Ok(());
    }

    for (document, _) in &converted {
        let backup = migration_backup(document);
        if backup.exists() {
            return Err(format!(
                "backup already exists: {}; move it before migrating again",
                backup.display()
            ));
        }
    }

    let temporaries = converted
        .iter()
        .map(|(document, _)| migration_temporary(document))
        .collect::<Vec<_>>();
    for (index, ((document, output), temporary)) in converted.iter().zip(&temporaries).enumerate() {
        let result = std::fs::write(temporary, output).and_then(|()| {
            let permissions = std::fs::metadata(document)?.permissions();
            std::fs::set_permissions(temporary, permissions)
        });
        if let Err(error) = result {
            cleanup_temporaries(&temporaries[..=index]);
            return Err(format!(
                "cannot prepare migrated {}: {error}",
                document.display()
            ));
        }
    }

    let mut backed_up = 0;
    for (document, _) in &converted {
        let backup = migration_backup(document);
        if let Err(error) = std::fs::rename(document, &backup) {
            cleanup_temporaries(&temporaries);
            let rollback = rollback_documents(&converted[..backed_up]);
            return Err(format!(
                "cannot create {}: {error}; {rollback}",
                backup.display()
            ));
        }
        backed_up += 1;
    }

    for (index, ((document, _), temporary)) in converted.iter().zip(&temporaries).enumerate() {
        if let Err(error) = std::fs::rename(temporary, document) {
            cleanup_temporaries(&temporaries[index..]);
            let rollback = rollback_documents(&converted);
            return Err(format!(
                "cannot install migrated {}: {error}; {rollback}",
                document.display()
            ));
        }
    }

    let validation = (|| {
        let mut manager = ConfigManager::new(path.clone());
        manager.load().map_err(|error| error.to_string())
    })();
    if let Err(error) = validation {
        let rollback = rollback_documents(&converted);
        return Err(format!(
            "migrated configuration failed v4 validation: {error}; {rollback}"
        ));
    }

    println!(
        "migrated {} documents to schema v4; .v3.bak backups were created (comments are not retained)",
        converted.len()
    );
    println!(
        "recommended v4 defaults: profile=balanced, screen_inhibit=while-active, external_boost=false, idle_view=none, observation_lease_seconds=300, battery=20/25% low and 10/15% critical"
    );
    println!(
        "note: v3 external_prevents_idle/hysteresis, Agent brightness protection, CPU hints, and multiplier fields have no v4 equivalent"
    );
    Ok(())
}

fn document_schema(path: &Path, text: &str) -> Result<u64, String> {
    if path.extension().and_then(|value| value.to_str()) == Some("json") {
        serde_json::from_str::<serde_json::Value>(text)
            .map_err(|error| format!("cannot parse JSON {}: {error}", path.display()))?
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("{} has no integer schema_version", path.display()))
    } else {
        toml::from_str::<toml::Value>(text)
            .map_err(|error| format!("cannot parse TOML {}: {error}", path.display()))?
            .get("schema_version")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| {
                format!(
                    "{} has no non-negative integer schema_version",
                    path.display()
                )
            })
    }
}

fn validate_migrated_document(path: &Path, text: &str, root: bool) -> Result<(), String> {
    let version = if root {
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            serde_json::from_str::<AppConfig>(text)
                .map_err(|error| format!("invalid migrated JSON {}: {error}", path.display()))?
                .schema_version
        } else {
            toml::from_str::<AppConfig>(text)
                .map_err(|error| format!("invalid migrated TOML {}: {error}", path.display()))?
                .schema_version
        }
    } else if path.extension().and_then(|value| value.to_str()) == Some("json") {
        serde_json::from_str::<ConfigFragment>(text)
            .map_err(|error| format!("invalid migrated JSON {}: {error}", path.display()))?
            .schema_version
    } else {
        toml::from_str::<ConfigFragment>(text)
            .map_err(|error| format!("invalid migrated TOML {}: {error}", path.display()))?
            .schema_version
    };
    if version == CONFIG_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(format!(
            "invalid migrated schema in {}: expected {CONFIG_SCHEMA_VERSION}, got {version}",
            path.display()
        ))
    }
}

fn migration_backup(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.v3.bak", path.display()))
}

fn migration_temporary(path: &Path) -> PathBuf {
    PathBuf::from(format!(
        "{}.v4-migrate-{}.tmp",
        path.display(),
        std::process::id()
    ))
}

fn cleanup_temporaries(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

fn rollback_documents(documents: &[(PathBuf, String)]) -> String {
    let mut failures = Vec::new();
    for (document, _) in documents.iter().rev() {
        let backup = migration_backup(document);
        if !backup.exists() {
            continue;
        }
        if document.exists() {
            if let Err(error) = std::fs::remove_file(document) {
                failures.push(format!("cannot remove {}: {error}", document.display()));
                continue;
            }
        }
        if let Err(error) = std::fs::rename(&backup, document) {
            failures.push(format!(
                "cannot restore {} from {}: {error}",
                document.display(),
                backup.display()
            ));
        }
    }
    if failures.is_empty() {
        "original documents restored".into()
    } else {
        format!("rollback incomplete: {}", failures.join("; "))
    }
}

fn migrate_document(path: &Path, text: &str) -> Result<String, String> {
    if path.extension().and_then(|value| value.to_str()) == Some("json") {
        let mut value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| format!("cannot parse legacy JSON {}: {error}", path.display()))?;
        migrate_json_value(&mut value, path)?;
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
    } else {
        let mut value: toml::Value = toml::from_str(text)
            .map_err(|error| format!("cannot parse legacy TOML {}: {error}", path.display()))?;
        migrate_toml_value(&mut value, path)?;
        toml::to_string_pretty(&value).map_err(|error| error.to_string())
    }
}

fn migrate_toml_value(value: &mut toml::Value, path: &Path) -> Result<(), String> {
    let root = value
        .as_table_mut()
        .ok_or_else(|| format!("legacy document {} must be a table", path.display()))?;
    match root.get("schema_version").and_then(toml::Value::as_integer) {
        Some(3) => {}
        Some(4) => return Err(format!("{} is already schema v4", path.display())),
        other => return Err(format!("{} is not schema v3 ({other:?})", path.display())),
    }
    root.insert("schema_version".into(), toml::Value::Integer(4));
    if let Some(runtime) = root.get_mut("runtime").and_then(toml::Value::as_table_mut) {
        migrate_toml_runtime(runtime)?;
    }
    if let Some(cards) = root.get_mut("cards").and_then(toml::Value::as_array_mut) {
        for card in cards {
            if let Some(runtime) = card
                .as_table_mut()
                .and_then(|card| card.get_mut("runtime"))
                .and_then(toml::Value::as_table_mut)
            {
                migrate_toml_card_runtime(runtime)?;
            }
        }
    }
    Ok(())
}

fn migrated_profile(idle_saving: Option<bool>, saving: Option<&str>) -> &'static str {
    if idle_saving == Some(false) {
        "performance"
    } else if saving == Some("aggressive") {
        "eco"
    } else {
        "balanced"
    }
}

fn migrated_idle_view(display: &str) -> &'static str {
    match display {
        "minimal" => "minimal",
        "dim" => "dim",
        _ => "none",
    }
}

fn migrated_workload(class: &str) -> Option<&'static str> {
    match class {
        "auto" => Some("auto"),
        "system-realtime" | "network-rate" => Some("live"),
        "command" | "http" => Some("expensive"),
        "network-status" | "file" | "static" => Some("event"),
        "battery-thermal" => Some("normal"),
        _ => None,
    }
}

fn migrate_toml_runtime(runtime: &mut toml::map::Map<String, toml::Value>) -> Result<(), String> {
    for field in [
        "keep_screen_on",
        "idle_power_saving",
        "external_realtime",
        "external_prevents_idle",
        "codex_keep_bright",
        "codex_completion_sound",
        "cpu_activity_hint",
    ] {
        validate_toml_field(runtime, field, "boolean", toml::Value::is_bool)?;
    }
    for field in [
        "external_sample_seconds",
        "external_enter_samples",
        "external_exit_samples",
        "codex_protection_minutes",
        "codex_attention_seconds",
    ] {
        validate_toml_field(runtime, field, "non-negative integer", |value| {
            value.as_integer().is_some_and(|value| value >= 0)
        })?;
    }
    validate_toml_field(
        runtime,
        "refresh_saving_strength",
        "`mild`, `balanced`, or `aggressive`",
        |value| matches!(value.as_str(), Some("mild" | "balanced" | "aggressive")),
    )?;
    validate_toml_field(runtime, "idle_display", "`dim` or `minimal`", |value| {
        matches!(value.as_str(), Some("dim" | "minimal"))
    })?;
    let keep = runtime
        .remove("keep_screen_on")
        .and_then(|value| value.as_bool());
    let saving = runtime
        .remove("refresh_saving_strength")
        .and_then(|value| value.as_str().map(str::to_owned));
    let idle_saving = runtime
        .remove("idle_power_saving")
        .and_then(|value| value.as_bool());
    let external_realtime = runtime
        .remove("external_realtime")
        .and_then(|value| value.as_bool());
    runtime.remove("external_prevents_idle");
    let display = runtime
        .remove("idle_display")
        .and_then(|value| value.as_str().map(str::to_owned));
    if let Some(sample) = runtime.remove("external_sample_seconds") {
        runtime.insert("power_sample_seconds".into(), sample);
    }
    if let Some(attention) = runtime.remove("codex_attention_seconds") {
        runtime.insert("agent_attention_seconds".into(), attention);
    }
    if let Some(sound) = runtime.remove("codex_completion_sound") {
        runtime.insert("agent_completion_sound".into(), sound);
    }
    runtime.remove("external_enter_samples");
    runtime.remove("external_exit_samples");
    runtime.remove("codex_keep_bright");
    runtime.remove("codex_protection_minutes");
    runtime.remove("cpu_activity_hint");
    if idle_saving.is_some() || saving.is_some() {
        runtime.insert(
            "profile".into(),
            toml::Value::String(migrated_profile(idle_saving, saving.as_deref()).into()),
        );
    }
    if let Some(keep) = keep {
        runtime.insert(
            "screen_inhibit".into(),
            toml::Value::String(if keep { "while-mapped" } else { "never" }.into()),
        );
    }
    if let Some(external_realtime) = external_realtime {
        runtime.insert(
            "external_boost".into(),
            toml::Value::Boolean(external_realtime),
        );
    }
    if let Some(display) = display {
        runtime.insert(
            "idle_view".into(),
            toml::Value::String(migrated_idle_view(&display).into()),
        );
    }
    Ok(())
}

fn validate_toml_field(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
    expected: &str,
    valid: impl Fn(&toml::Value) -> bool,
) -> Result<(), String> {
    if let Some(value) = table.get(field) {
        if !valid(value) {
            return Err(format!(
                "legacy field `{field}` must be {expected}, got {value:?}"
            ));
        }
    }
    Ok(())
}

fn migrate_toml_card_runtime(
    runtime: &mut toml::map::Map<String, toml::Value>,
) -> Result<(), String> {
    validate_toml_field(runtime, "class", "a known string", |value| {
        value.as_str().and_then(migrated_workload).is_some()
    })?;
    validate_toml_field(runtime, "idle_behavior", "`throttle` or `pause`", |value| {
        matches!(value.as_str(), Some("throttle" | "pause"))
    })?;
    validate_toml_field(runtime, "idle_multiplier", "number", |value| {
        value.is_float() || value.is_integer()
    })?;
    validate_toml_field(
        runtime,
        "external_realtime",
        "boolean",
        toml::Value::is_bool,
    )?;
    validate_toml_field(runtime, "realtime_multiplier", "number", |value| {
        value.is_float() || value.is_integer()
    })?;
    let class = runtime
        .remove("class")
        .and_then(|value| value.as_str().map(str::to_owned));
    let idle = runtime
        .remove("idle_behavior")
        .and_then(|value| value.as_str().map(str::to_owned));
    runtime.remove("idle_multiplier");
    runtime.remove("external_realtime");
    runtime.remove("realtime_multiplier");
    if let Some(class) = class {
        runtime.insert(
            "workload".into(),
            toml::Value::String(
                migrated_workload(&class)
                    .expect("legacy class was validated")
                    .into(),
            ),
        );
    }
    if let Some(idle) = idle {
        runtime.insert("idle_behavior".into(), toml::Value::String(idle));
    }
    Ok(())
}

fn migrate_json_value(value: &mut serde_json::Value, path: &Path) -> Result<(), String> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| format!("legacy document {} must be an object", path.display()))?;
    match root
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(3) => {}
        Some(4) => return Err(format!("{} is already schema v4", path.display())),
        other => return Err(format!("{} is not schema v3 ({other:?})", path.display())),
    }
    root.insert("schema_version".into(), serde_json::Value::from(4));
    if let Some(runtime) = root
        .get_mut("runtime")
        .and_then(serde_json::Value::as_object_mut)
    {
        for field in [
            "keep_screen_on",
            "idle_power_saving",
            "external_realtime",
            "external_prevents_idle",
            "codex_keep_bright",
            "codex_completion_sound",
            "cpu_activity_hint",
        ] {
            validate_json_field(runtime, field, "boolean", serde_json::Value::is_boolean)?;
        }
        for field in [
            "external_sample_seconds",
            "external_enter_samples",
            "external_exit_samples",
            "codex_protection_minutes",
            "codex_attention_seconds",
        ] {
            validate_json_field(
                runtime,
                field,
                "non-negative integer",
                serde_json::Value::is_u64,
            )?;
        }
        validate_json_field(
            runtime,
            "refresh_saving_strength",
            "`mild`, `balanced`, or `aggressive`",
            |value| matches!(value.as_str(), Some("mild" | "balanced" | "aggressive")),
        )?;
        validate_json_field(runtime, "idle_display", "`dim` or `minimal`", |value| {
            matches!(value.as_str(), Some("dim" | "minimal"))
        })?;
        let keep = runtime
            .remove("keep_screen_on")
            .and_then(|value| value.as_bool());
        let saving = runtime
            .remove("refresh_saving_strength")
            .and_then(|value| value.as_str().map(str::to_owned));
        let idle_saving = runtime
            .remove("idle_power_saving")
            .and_then(|value| value.as_bool());
        let external_realtime = runtime
            .remove("external_realtime")
            .and_then(|value| value.as_bool());
        runtime.remove("external_prevents_idle");
        let display = runtime
            .remove("idle_display")
            .and_then(|value| value.as_str().map(str::to_owned));
        if let Some(sample) = runtime.remove("external_sample_seconds") {
            runtime.insert("power_sample_seconds".into(), sample);
        }
        if let Some(attention) = runtime.remove("codex_attention_seconds") {
            runtime.insert("agent_attention_seconds".into(), attention);
        }
        if let Some(sound) = runtime.remove("codex_completion_sound") {
            runtime.insert("agent_completion_sound".into(), sound);
        }
        runtime.remove("external_enter_samples");
        runtime.remove("external_exit_samples");
        runtime.remove("codex_keep_bright");
        runtime.remove("codex_protection_minutes");
        runtime.remove("cpu_activity_hint");
        if idle_saving.is_some() || saving.is_some() {
            runtime.insert(
                "profile".into(),
                serde_json::Value::String(migrated_profile(idle_saving, saving.as_deref()).into()),
            );
        }
        if let Some(keep) = keep {
            runtime.insert(
                "screen_inhibit".into(),
                serde_json::Value::String(if keep { "while-mapped" } else { "never" }.into()),
            );
        }
        if let Some(external_realtime) = external_realtime {
            runtime.insert(
                "external_boost".into(),
                serde_json::Value::Bool(external_realtime),
            );
        }
        if let Some(display) = display {
            runtime.insert(
                "idle_view".into(),
                serde_json::Value::String(migrated_idle_view(&display).into()),
            );
        }
    }
    if let Some(cards) = root
        .get_mut("cards")
        .and_then(serde_json::Value::as_array_mut)
    {
        for card in cards {
            if let Some(runtime) = card
                .get_mut("runtime")
                .and_then(serde_json::Value::as_object_mut)
            {
                validate_json_field(runtime, "class", "a known string", |value| {
                    value.as_str().and_then(migrated_workload).is_some()
                })?;
                validate_json_field(runtime, "idle_behavior", "`throttle` or `pause`", |value| {
                    matches!(value.as_str(), Some("throttle" | "pause"))
                })?;
                validate_json_field(
                    runtime,
                    "idle_multiplier",
                    "number",
                    serde_json::Value::is_number,
                )?;
                validate_json_field(
                    runtime,
                    "external_realtime",
                    "boolean",
                    serde_json::Value::is_boolean,
                )?;
                validate_json_field(
                    runtime,
                    "realtime_multiplier",
                    "number",
                    serde_json::Value::is_number,
                )?;
                let class = runtime
                    .remove("class")
                    .and_then(|value| value.as_str().map(str::to_owned));
                let idle = runtime
                    .remove("idle_behavior")
                    .and_then(|value| value.as_str().map(str::to_owned));
                runtime.remove("idle_multiplier");
                runtime.remove("external_realtime");
                runtime.remove("realtime_multiplier");
                if let Some(class) = class {
                    runtime.insert(
                        "workload".into(),
                        serde_json::Value::String(
                            migrated_workload(&class)
                                .expect("legacy class was validated")
                                .into(),
                        ),
                    );
                }
                if let Some(idle) = idle {
                    runtime.insert("idle_behavior".into(), serde_json::Value::String(idle));
                }
            }
        }
    }
    Ok(())
}

fn validate_json_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    expected: &str,
    valid: impl Fn(&serde_json::Value) -> bool,
) -> Result<(), String> {
    if let Some(value) = object.get(field) {
        if !valid(value) {
            return Err(format!(
                "legacy field `{field}` must be {expected}, got {value}"
            ));
        }
    }
    Ok(())
}

fn single_optional_path(arguments: &[String], command: &str) -> Result<PathBuf, String> {
    match arguments {
        [] => Ok(config_path()),
        [path] => Ok(PathBuf::from(path)),
        _ => Err(format!("usage: pulsedeck config {command} [CONFIG_FILE]")),
    }
}

#[derive(Default)]
struct AddOptions {
    id: Option<String>,
    title: Option<String>,
    page: Option<String>,
    module: Option<String>,
    renderer: Option<String>,
    refresh: Option<String>,
    order: Option<String>,
    icon: Option<String>,
    description: Option<String>,
    config: Option<String>,
    disabled: bool,
    positional: Vec<String>,
    command: Vec<String>,
}

fn add(arguments: &[String]) -> Result<(), String> {
    let kind = arguments
        .first()
        .ok_or_else(|| format!("missing card source kind\n\n{USAGE}"))?;
    let options = parse_add_options(&arguments[1..])?;
    let source = build_source(kind, &options)?;
    let id = options
        .id
        .clone()
        .or_else(|| {
            (kind == "builtin")
                .then(|| options.positional.first().cloned())
                .flatten()
        })
        .ok_or_else(|| "missing --id ID".to_string())?;
    validate_identifier("card id", &id)?;
    let renderer = parse_renderer(options.renderer.as_deref().unwrap_or("value"))?;
    let refresh_interval = options
        .refresh
        .as_deref()
        .map(parse_duration)
        .transpose()?
        .unwrap_or(30);
    let order = options
        .order
        .as_deref()
        .map(|value| {
            value
                .parse::<i32>()
                .map_err(|_| format!("invalid --order value: {value}"))
        })
        .transpose()?
        .unwrap_or(0);
    let card = CardConfig {
        id: id.clone(),
        title: options.title.clone().unwrap_or_else(|| id.clone()),
        page: options.page.clone().unwrap_or_else(|| "monitor".into()),
        order,
        renderer,
        refresh_interval,
        enabled: !options.disabled,
        icon: options.icon.clone(),
        description: options.description.clone(),
        source: Some(source),
        display: None,
        cache_ttl_seconds: None,
        schedule: None,
        click_action: None,
        kind: None,
        plugin: None,
        runtime: CardRuntimeConfig::default(),
    };

    let config = options
        .config
        .map(PathBuf::from)
        .unwrap_or_else(config_path);
    let mut manager = ConfigManager::new(config.clone());
    manager
        .load()
        .map_err(|error| format!("cannot load {}: {error}", config.display()))?;
    if !manager
        .config()
        .pages
        .iter()
        .any(|page| page.id == card.page)
    {
        return Err(format!(
            "card page `{}` does not exist in {}",
            card.page,
            config.display()
        ));
    }
    let target = select_module(&manager, options.module.as_deref())?;
    let path = manager
        .upsert_module_card(&target.file_name, target.new_name.as_deref(), card)
        .map_err(|error| format!("cannot update selected configuration: {error}"))?;
    crate::plugins::validate_config(manager.config())
        .map_err(|error| format!("generated configuration is invalid: {error}"))?;
    println!(
        "saved card {id} to {}{}",
        path.display(),
        if target.replaces_existing {
            " (this module overrides earlier matching ids)"
        } else {
            ""
        }
    );
    Ok(())
}

struct ModuleTarget {
    file_name: String,
    new_name: Option<String>,
    replaces_existing: bool,
}

fn select_module(manager: &ConfigManager, requested: Option<&str>) -> Result<ModuleTarget, String> {
    let modules = manager.loaded_modules();
    if let Some(requested) = requested {
        if let Some(module) = modules
            .iter()
            .find(|module| module_matches(module, requested))
        {
            return Ok(ModuleTarget {
                file_name: module.file_name.clone(),
                new_name: None,
                replaces_existing: module.replace_existing,
            });
        }
        return new_module_target(requested);
    }

    println!("Choose a configuration file for the new card:");
    for (index, module) in modules.iter().enumerate() {
        let name = module
            .name
            .as_deref()
            .map(|name| format!(" · {name}"))
            .unwrap_or_default();
        let overlay = if module.replace_existing {
            " · override"
        } else {
            ""
        };
        println!("  {}) {}{name}{overlay}", index + 1, module.file_name);
    }
    println!("  {}) Create a new configuration file", modules.len() + 1);
    print!("Selection: ");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let selection = read_line()?;
    let selection = selection
        .parse::<usize>()
        .map_err(|_| "selection must be a number".to_string())?;
    if let Some(module) = selection
        .checked_sub(1)
        .and_then(|index| modules.get(index))
    {
        return Ok(ModuleTarget {
            file_name: module.file_name.clone(),
            new_name: None,
            replaces_existing: module.replace_existing,
        });
    }
    if selection != modules.len() + 1 {
        return Err("selection is out of range".into());
    }
    print!("New file name or module name: ");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let requested = read_line()?;
    new_module_target(&requested)
}

fn module_matches(module: &ConfigModuleInfo, requested: &str) -> bool {
    if module.file_name == requested || module.name.as_deref() == Some(requested) {
        return true;
    }
    let stem = Path::new(&module.file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    stem == requested
        || stem.split_once('-').is_some_and(|(prefix, name)| {
            prefix.chars().all(|character| character.is_ascii_digit()) && name == requested
        })
}

fn new_module_target(requested: &str) -> Result<ModuleTarget, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("new configuration file name cannot be empty".into());
    }
    let has_supported_extension = matches!(
        Path::new(requested)
            .extension()
            .and_then(|value| value.to_str()),
        Some("toml" | "json")
    );
    let (file_name, name) = if has_supported_extension {
        if Path::new(requested).components().count() != 1 {
            return Err("the new module must be a file name inside config.d".into());
        }
        let stem = Path::new(requested)
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "invalid module file name".to_string())?;
        (requested.to_string(), human_module_name(stem))
    } else {
        validate_identifier("module name", requested)?;
        (format!("50-{requested}.toml"), requested.to_string())
    };
    Ok(ModuleTarget {
        file_name,
        new_name: Some(name),
        replaces_existing: true,
    })
}

fn human_module_name(stem: &str) -> String {
    stem.split_once('-')
        .filter(|(prefix, _)| prefix.chars().all(|character| character.is_ascii_digit()))
        .map(|(_, name)| name)
        .unwrap_or(stem)
        .to_string()
}

fn read_line() -> Result<String, String> {
    let mut value = String::new();
    if io::stdin()
        .read_line(&mut value)
        .map_err(|error| error.to_string())?
        == 0
    {
        return Err("no selection received; use --module NAME_OR_FILE in scripts".into());
    }
    Ok(value.trim().to_string())
}

fn parse_add_options(arguments: &[String]) -> Result<AddOptions, String> {
    let mut result = AddOptions::default();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--" {
            result.command = arguments[index + 1..].to_vec();
            break;
        }
        if argument == "--disabled" {
            result.disabled = true;
            index += 1;
            continue;
        }
        if argument.starts_with("--") {
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("missing value for {argument}"))?
                .clone();
            match argument.as_str() {
                "--id" => result.id = Some(value),
                "--title" => result.title = Some(value),
                "--page" => result.page = Some(value),
                "--module" => result.module = Some(value),
                "--renderer" => result.renderer = Some(value),
                "--refresh" => result.refresh = Some(value),
                "--order" => result.order = Some(value),
                "--icon" => result.icon = Some(value),
                "--description" => result.description = Some(value),
                "--config" => result.config = Some(value),
                _ => return Err(format!("unknown add option: {argument}")),
            }
            index += 2;
        } else {
            result.positional.push(argument.clone());
            index += 1;
        }
    }
    Ok(result)
}

fn build_source(kind: &str, options: &AddOptions) -> Result<SourceConfig, String> {
    match kind {
        "builtin" => {
            let metric = one_positional(kind, options)?;
            if !matches!(
                metric.as_str(),
                "cpu"
                    | "memory"
                    | "uptime"
                    | "battery_capacity"
                    | "battery_temperature"
                    | "power"
                    | "network"
                    | "load_average"
                    | "swap"
                    | "process_count"
                    | "cpu_temperature"
                    | "filesystem"
                    | "network_traffic"
            ) {
                return Err(format!("unknown builtin metric: {metric}"));
            }
            Ok(SourceConfig::Builtin(metric))
        }
        "file" => one_positional(kind, options).map(|path| {
            SourceConfig::File(FileSourceConfig {
                path,
                first_line: true,
            })
        }),
        "http" => one_positional(kind, options).map(|url| {
            SourceConfig::Http(HttpSourceConfig {
                url,
                method: None,
                headers: None,
                body: None,
                timeout_seconds: 10,
                max_output_bytes: 20_000,
                parser: None,
            })
        }),
        "text" => one_positional(kind, options).map(SourceConfig::Text),
        "command" => {
            if !options.positional.is_empty() {
                return Err("command arguments must follow --".into());
            }
            if options.command.is_empty() {
                return Err("command source requires -- PROGRAM [ARG ...]".into());
            }
            Ok(SourceConfig::Command(CommandSourceConfig {
                run: options.command.clone(),
                timeout_seconds: 10,
                max_output_bytes: 20_000,
                reverse_lines: false,
                subtitle_lines: 0,
            }))
        }
        _ => Err(format!("unknown card source kind: {kind}")),
    }
}

fn one_positional(kind: &str, options: &AddOptions) -> Result<String, String> {
    match options.positional.as_slice() {
        [value] => Ok(value.clone()),
        [] => Err(format!("{kind} source requires one value")),
        _ => Err(format!("{kind} source accepts exactly one value")),
    }
}

fn validate_identifier(label: &str, value: &str) -> Result<(), String> {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        Ok(())
    } else {
        Err(format!("invalid {label}: {value}"))
    }
}

fn parse_renderer(value: &str) -> Result<RendererKind, String> {
    match value {
        "text" => Ok(RendererKind::Text),
        "value" => Ok(RendererKind::Value),
        "progress" => Ok(RendererKind::Progress),
        "status" => Ok(RendererKind::Status),
        "list" => Ok(RendererKind::List),
        "composite" => Ok(RendererKind::Composite),
        _ => Err(format!("invalid renderer: {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_module_names_support_existing_and_new_files() {
        let existing = ConfigModuleInfo {
            file_name: "50-personal.toml".into(),
            name: Some("personal".into()),
            replace_existing: true,
        };
        assert!(module_matches(&existing, "personal"));
        assert!(module_matches(&existing, "50-personal"));
        assert!(module_matches(&existing, "50-personal.toml"));

        let generated = new_module_target("travel").unwrap();
        assert_eq!(generated.file_name, "50-travel.toml");
        assert_eq!(generated.new_name.as_deref(), Some("travel"));
        assert!(generated.replaces_existing);
    }

    #[test]
    fn module_targets_cannot_escape_config_directory() {
        assert!(new_module_target("../personal.toml").is_err());
        assert!(new_module_target("personal.yaml").is_err());
    }

    #[test]
    fn v3_runtime_migration_produces_strict_v4_fields() {
        let legacy = r#"
schema_version = 3

[runtime]
keep_screen_on = true
idle_power_saving = true
refresh_saving_strength = "mild"
external_realtime = false
external_prevents_idle = true
external_sample_seconds = 9
external_enter_samples = 3
external_exit_samples = 2
codex_keep_bright = true
codex_protection_minutes = 60
codex_attention_seconds = 20
codex_completion_sound = false
cpu_activity_hint = true
idle_display = "dim"
"#;
        let output = migrate_document(Path::new("config.toml"), legacy).unwrap();
        let config: AppConfig = toml::from_str(&output).unwrap();
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(
            config.runtime.profile,
            crate::core::config::RuntimeProfile::Balanced
        );
        assert_eq!(
            config.runtime.screen_inhibit,
            crate::core::config::ScreenInhibitMode::WhileMapped
        );
        assert!(!config.runtime.external_boost);
        assert_eq!(config.runtime.power_sample_seconds, 9);
        assert_eq!(config.runtime.agent_attention_seconds, 20);
        assert!(!config.runtime.agent_completion_sound);
        assert_eq!(
            config.runtime.idle_view,
            crate::core::config::IdleViewMode::Dim
        );
        for obsolete in [
            "keep_screen_on",
            "idle_power_saving",
            "refresh_saving_strength",
            "external_realtime",
            "external_prevents_idle",
            "external_enter_samples",
            "external_exit_samples",
            "codex_keep_bright",
            "codex_protection_minutes",
            "codex_attention_seconds",
            "codex_completion_sound",
            "cpu_activity_hint",
            "idle_display",
        ] {
            assert!(
                !output.contains(obsolete),
                "obsolete field remained: {obsolete}"
            );
        }
    }

    #[test]
    fn json_runtime_migration_uses_the_same_v4_mapping() {
        let legacy = r#"{
  "schema_version": 3,
  "runtime": {
    "keep_screen_on": false,
    "idle_power_saving": false,
    "codex_attention_seconds": 7,
    "codex_completion_sound": true
  }
}"#;
        let output = migrate_document(Path::new("config.json"), legacy).unwrap();
        let config: AppConfig = serde_json::from_str(&output).unwrap();
        assert_eq!(
            config.runtime.profile,
            crate::core::config::RuntimeProfile::Performance
        );
        assert_eq!(
            config.runtime.screen_inhibit,
            crate::core::config::ScreenInhibitMode::Never
        );
        assert_eq!(config.runtime.agent_attention_seconds, 7);
        assert!(config.runtime.agent_completion_sound);
    }

    #[test]
    fn fragment_migration_does_not_invent_unrelated_overrides() {
        let legacy = r#"
schema_version = 3
name = "power-only"
replace_existing = true

[runtime]
external_realtime = false
"#;
        let output = migrate_document(Path::new("50-power.toml"), legacy).unwrap();
        let value: toml::Value = toml::from_str(&output).unwrap();
        let runtime = value["runtime"].as_table().unwrap();
        assert_eq!(runtime.len(), 1);
        assert_eq!(runtime["external_boost"].as_bool(), Some(false));
        let _: ConfigFragment = toml::from_str(&output).unwrap();
    }

    #[test]
    fn malformed_legacy_values_are_rejected_instead_of_defaulted() {
        let bad_toml = r#"
schema_version = 3
[runtime]
keep_screen_on = "yes"
"#;
        assert!(migrate_document(Path::new("config.toml"), bad_toml)
            .unwrap_err()
            .contains("keep_screen_on"));

        let bad_json = r#"{
  "schema_version": 3,
  "runtime": {"codex_attention_seconds": "soon"}
}"#;
        assert!(migrate_document(Path::new("config.json"), bad_json)
            .unwrap_err()
            .contains("codex_attention_seconds"));

        let unknown_saving = r#"
schema_version = 3
[runtime]
refresh_saving_strength = "typo"
"#;
        assert!(migrate_document(Path::new("config.toml"), unknown_saving)
            .unwrap_err()
            .contains("refresh_saving_strength"));

        let unknown_display = r#"{
  "schema_version": 3,
  "runtime": {"idle_display": "ambient"}
}"#;
        assert!(migrate_document(Path::new("config.json"), unknown_display)
            .unwrap_err()
            .contains("idle_display"));

        let unknown_class = r#"
schema_version = 3
[[cards]]
id = "bad"
title = "Bad"
page = "monitor"
[cards.runtime]
class = "burst"
"#;
        assert!(migrate_document(Path::new("config.toml"), unknown_class)
            .unwrap_err()
            .contains("class"));
    }

    #[test]
    fn card_runtime_class_migrates_to_workload_without_legacy_tuning_fields() {
        let legacy = r#"
schema_version = 3

[[cards]]
id = "remote"
title = "Remote"
page = "monitor"
renderer = "text"
refresh = "1m"
source = { text = "ok" }

[cards.runtime]
class = "http"
idle_behavior = "throttle"
idle_multiplier = 8.0
external_realtime = false
realtime_multiplier = 0.75
"#;
        let output = migrate_document(Path::new("config.toml"), legacy).unwrap();
        let config: AppConfig = toml::from_str(&output).unwrap();
        assert_eq!(
            config.cards[0].runtime.workload,
            crate::core::config::CardWorkload::Expensive
        );
        assert_eq!(
            config.cards[0].runtime.idle_behavior,
            crate::core::config::CardWorkBehavior::Throttle
        );
        assert!(!output.contains("idle_multiplier"));
        assert!(!output.contains("realtime_multiplier"));
    }
}
