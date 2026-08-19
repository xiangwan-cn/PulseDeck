use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::core::config::{
    config_path, parse_duration, CardConfig, CardRuntimeConfig, CommandSourceConfig, ConfigManager,
    ConfigModuleInfo, FileSourceConfig, HttpSourceConfig, SourceConfig,
};
use crate::model::card_model::RendererKind;

const USAGE: &str = "\
PulseDeck configuration tools

  pulsedeck config check [CONFIG_FILE]
  pulsedeck config format [CONFIG_FILE]
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
}
