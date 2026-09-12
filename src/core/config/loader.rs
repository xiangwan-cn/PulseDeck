use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::{
    AppConfig, CardConfig, CardTransitionConfig, CardVisualStateConfig, ConfigFragment,
    DisplayConfig, HttpSourceConfig, ParserConfig, ParserKind, SourceConfig, CONFIG_SCHEMA_VERSION,
};
use crate::core::error::AppError;

const MAX_CONFIG_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONFIG_MODULES: usize = 128;
const MAX_PAGES: usize = 32;
const MAX_CARDS: usize = 512;
const MAX_ACTIONS: usize = 256;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_SOURCE_TEXT_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_DURATION_SECONDS: u64 = 30 * 24 * 60 * 60;
const MAX_EXTERNAL_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Clone)]
struct LoadedFragment {
    path: PathBuf,
    config: ConfigFragment,
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigModuleInfo {
    pub file_name: String,
    pub name: Option<String>,
    pub replace_existing: bool,
}

pub struct ConfigManager {
    path: PathBuf,
    config: AppConfig,
    root: AppConfig,
    fragments: Vec<LoadedFragment>,
}

impl ConfigManager {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            config: AppConfig::default(),
            root: AppConfig::default(),
            fragments: Vec::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn module_dir(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("config.d")
    }

    pub fn loaded_module_count(&self) -> usize {
        self.fragments.len()
    }

    pub(crate) fn loaded_modules(&self) -> Vec<ConfigModuleInfo> {
        self.fragments
            .iter()
            .filter_map(|fragment| {
                Some(ConfigModuleInfo {
                    file_name: fragment.path.file_name()?.to_str()?.to_string(),
                    name: fragment.config.name.clone(),
                    replace_existing: fragment.config.replace_existing,
                })
            })
            .collect()
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// An empty root document may still acquire generated capability modules
    /// during startup. Those entries are not a replacement for the one parsed
    /// default-card registry; enabled module cards are layered on top.
    pub fn uses_default_card_registry(&self) -> bool {
        self.root.cards.is_empty()
    }

    /// Generated plugin/capability pages are layered over the shipped default
    /// pages when the root document does not define its own page registry.
    pub fn uses_default_page_registry(&self) -> bool {
        self.root.pages.is_empty()
    }

    pub fn config_mut(&mut self) -> &mut AppConfig {
        &mut self.config
    }

    pub fn load(&mut self) -> Result<(), AppError> {
        if !self.path.exists() {
            return Err(AppError::ConfigNotFound(self.path.clone()));
        }

        let root: AppConfig = read_document(&self.path)?;
        validate_schema(root.schema_version, &self.path)?;

        let mut fragments = Vec::new();
        for path in discover_fragment_paths(&self.module_dir())? {
            let config: ConfigFragment = read_document(&path)?;
            validate_schema(config.schema_version, &path)?;
            validate_fragment_shape(&path, &config)?;
            fragments.push(LoadedFragment { path, config });
        }

        let merged = merge_config(&self.path, &root, &fragments)?;
        self.root = root;
        self.fragments = fragments;
        self.config = merged;

        Ok(())
    }

    /// Persist changes back to the document that owns each entry. New entries
    /// are placed in the main file; existing module entries remain modular.
    pub fn save(&mut self) -> Result<(), AppError> {
        validate_runtime(&self.path, &self.config)?;
        validate_structure(&self.path, &self.config)?;
        validate_card_assets(&self.path, &self.config)?;
        crate::plugins::validate_config(&self.config)?;
        let previous = merge_config(&self.path, &self.root, &self.fragments)?;
        let mut root = self.root.clone();
        let mut fragments = self.fragments.clone();

        if let Some(owner) = fragments
            .iter()
            .rposition(|fragment| fragment.config.app.is_some())
        {
            fragments[owner]
                .config
                .app
                .get_or_insert_default()
                .record_changes(&previous.app, &self.config.app);
        } else {
            root.app = self.config.app.clone();
        }
        if let Some(owner) = fragments
            .iter()
            .rposition(|fragment| fragment.config.ui.is_some())
        {
            fragments[owner]
                .config
                .ui
                .get_or_insert_default()
                .record_changes(&previous.ui, &self.config.ui);
        } else {
            root.ui = self.config.ui.clone();
        }
        if let Some(owner) = fragments
            .iter()
            .rposition(|fragment| fragment.config.runtime.is_some())
        {
            fragments[owner]
                .config
                .runtime
                .get_or_insert_default()
                .record_changes(&previous.runtime, &self.config.runtime);
        } else {
            root.runtime = self.config.runtime.clone();
        }

        let mut page_owners = HashMap::new();
        let mut card_owners = HashMap::new();
        let mut action_owners = HashMap::new();
        for (index, fragment) in fragments.iter().enumerate() {
            for page in &fragment.config.pages {
                page_owners.insert(page.id.clone(), index);
            }
            for card in &fragment.config.cards {
                card_owners.insert(card.id.clone(), index);
            }
            for action in &fragment.config.actions {
                action_owners.insert(action.id.clone(), index);
            }
        }
        sync_owned_entries(
            &mut root.pages,
            &self.config.pages,
            &page_owners,
            None,
            |page| &page.id,
        );
        sync_owned_entries(
            &mut root.cards,
            &self.config.cards,
            &card_owners,
            None,
            |card| &card.id,
        );
        sync_owned_entries(
            &mut root.actions,
            &self.config.actions,
            &action_owners,
            None,
            |action| &action.id,
        );
        for (index, fragment) in fragments.iter_mut().enumerate() {
            sync_owned_entries(
                &mut fragment.config.pages,
                &self.config.pages,
                &page_owners,
                Some(index),
                |page| &page.id,
            );
            sync_owned_entries(
                &mut fragment.config.cards,
                &self.config.cards,
                &card_owners,
                Some(index),
                |card| &card.id,
            );
            sync_owned_entries(
                &mut fragment.config.actions,
                &self.config.actions,
                &action_owners,
                Some(index),
                |action| &action.id,
            );
        }

        let mut page_ids = entry_ids(&root.pages, |page| &page.id);
        let mut card_ids = entry_ids(&root.cards, |card| &card.id);
        let mut action_ids = entry_ids(&root.actions, |action| &action.id);
        for fragment in &fragments {
            page_ids.extend(entry_ids(&fragment.config.pages, |page| &page.id));
            card_ids.extend(entry_ids(&fragment.config.cards, |card| &card.id));
            action_ids.extend(entry_ids(&fragment.config.actions, |action| &action.id));
        }
        root.pages.extend(
            self.config
                .pages
                .iter()
                .filter(|page| page_ids.insert(page.id.clone()))
                .cloned(),
        );
        root.cards.extend(
            self.config
                .cards
                .iter()
                .filter(|card| card_ids.insert(card.id.clone()))
                .cloned(),
        );
        root.actions.extend(
            self.config
                .actions
                .iter()
                .filter(|action| action_ids.insert(action.id.clone()))
                .cloned(),
        );

        write_if_changed(&self.path, &self.root, &root)?;
        for (previous, next) in self.fragments.iter().zip(&fragments) {
            write_if_changed(&next.path, &previous.config, &next.config)?;
        }
        self.root = root;
        self.fragments = fragments;
        Ok(())
    }

    /// Rewrite the root and every loaded module in the canonical compact
    /// schema. This is deliberately explicit because formatting removes
    /// comments while preserving configuration values and module ownership.
    pub(crate) fn format_documents(&mut self) -> Result<(), AppError> {
        write_document(&self.path, &self.root)?;
        for fragment in &self.fragments {
            write_document(&fragment.path, &fragment.config)?;
        }
        Ok(())
    }

    /// Insert or replace a card in a selected module. Existing modules keep
    /// their name and replacement policy; a newly generated module is a named
    /// personal overlay and therefore opts into intentional replacement.
    pub(crate) fn upsert_module_card(
        &mut self,
        file_name: &str,
        new_module_name: Option<&str>,
        card: super::CardConfig,
    ) -> Result<PathBuf, AppError> {
        validate_module_file_name(file_name)?;
        let path = self.module_dir().join(file_name);
        let mut fragments = self.fragments.clone();
        if let Some(fragment) = fragments.iter_mut().find(|fragment| fragment.path == path) {
            if let Some(existing) = fragment
                .config
                .cards
                .iter_mut()
                .find(|existing| existing.id == card.id)
            {
                *existing = card;
            } else {
                fragment.config.cards.push(card);
            }
        } else {
            fragments.push(LoadedFragment {
                path: path.clone(),
                config: ConfigFragment {
                    name: new_module_name.map(str::to_string),
                    replace_existing: true,
                    cards: vec![card],
                    ..ConfigFragment::default()
                },
            });
            fragments.sort_by(|left, right| left.path.cmp(&right.path));
        }

        for fragment in &fragments {
            validate_fragment_shape(&fragment.path, &fragment.config)?;
        }
        let merged = merge_config(&self.path, &self.root, &fragments)?;
        let next = fragments
            .iter()
            .find(|fragment| fragment.path == path)
            .expect("personal module is present");
        crate::core::config::ensure_private_directory(&self.module_dir())?;
        write_document(&path, &next.config)?;
        self.fragments = fragments;
        self.config = merged;
        Ok(path)
    }

    /// Add generated plugin defaults to their own module and immediately make
    /// them part of the merged runtime configuration.
    pub(crate) fn ensure_module(
        &mut self,
        file_name: &str,
        addition: ConfigFragment,
    ) -> Result<(), AppError> {
        validate_module_file_name(file_name)?;
        let file = Path::new(file_name);
        validate_schema(addition.schema_version, file)?;
        validate_fragment_shape(file, &addition)?;

        let path = self.module_dir().join(file);
        let mut fragments = self.fragments.clone();
        if let Some(fragment) = fragments.iter_mut().find(|fragment| fragment.path == path) {
            fragment.config.pages.extend(addition.pages);
            fragment.config.cards.extend(addition.cards);
            fragment.config.actions.extend(addition.actions);
        } else {
            fragments.push(LoadedFragment {
                path: path.clone(),
                config: addition,
            });
            fragments.sort_by(|left, right| left.path.cmp(&right.path));
        }

        let merged = merge_config(&self.path, &self.root, &fragments)?;
        let next = fragments
            .iter()
            .find(|fragment| fragment.path == path)
            .expect("new module is present");
        crate::core::config::ensure_private_directory(&self.module_dir())?;
        if let Some(previous) = self.fragments.iter().find(|fragment| fragment.path == path) {
            write_if_changed(&path, &previous.config, &next.config)?;
        } else {
            write_document(&path, &next.config)?;
        }
        self.fragments = fragments;
        self.config = merged;
        Ok(())
    }
}

fn validate_module_file_name(file_name: &str) -> Result<(), AppError> {
    let file = Path::new(file_name);
    if file.components().count() == 1
        && matches!(
            file.extension().and_then(|value| value.to_str()),
            Some("toml" | "json")
        )
    {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "invalid config module file name: {file_name}"
        )))
    }
}

fn read_document<T: DeserializeOwned>(path: &Path) -> Result<T, AppError> {
    let metadata = std::fs::metadata(path).map_err(|error| AppError::ConfigParse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if metadata.len() > MAX_CONFIG_DOCUMENT_BYTES {
        return Err(AppError::ConfigParse {
            path: path.to_path_buf(),
            message: format!(
                "configuration document exceeds {} bytes",
                MAX_CONFIG_DOCUMENT_BYTES
            ),
        });
    }
    let mut file = File::open(path).map_err(|error| AppError::ConfigParse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut content = String::with_capacity(metadata.len() as usize);
    file.read_to_string(&mut content)
        .map_err(|error| AppError::ConfigParse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    let parsed = if path.extension().and_then(|value| value.to_str()) == Some("json") {
        serde_json::from_str(&content).map_err(|error| error.to_string())
    } else {
        toml::from_str(&content).map_err(|error| error.to_string())
    };
    parsed.map_err(|message| AppError::ConfigParse {
        path: path.to_path_buf(),
        message,
    })
}

fn validate_schema(version: u32, path: &Path) -> Result<(), AppError> {
    if version == CONFIG_SCHEMA_VERSION {
        return Ok(());
    }
    Err(AppError::ConfigParse {
        path: path.to_path_buf(),
        message: format!("unsupported schema_version {version}; expected {CONFIG_SCHEMA_VERSION}"),
    })
}

fn validate_fragment_shape(path: &Path, fragment: &ConfigFragment) -> Result<(), AppError> {
    if let Some(name) = fragment.name.as_deref() {
        validate_text(path, "module.name", name, 256)?;
    }
    if fragment.pages.len() > MAX_PAGES
        || fragment.cards.len() > MAX_CARDS
        || fragment.actions.len() > MAX_ACTIONS
    {
        return Err(config_error(
            path,
            format!(
                "module entries exceed the supported limits (pages <= {MAX_PAGES}, cards <= {MAX_CARDS}, actions <= {MAX_ACTIONS})"
            ),
        ));
    }
    Ok(())
}

fn discover_fragment_paths(directory: &Path) -> Result<Vec<PathBuf>, AppError> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(directory).map_err(|error| AppError::ConfigParse {
        path: directory.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| AppError::ConfigParse {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?;
        let path = entry.path();
        let supported = matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("toml" | "json")
        );
        if supported && path.is_file() {
            paths.push(path);
            if paths.len() > MAX_CONFIG_MODULES {
                return Err(AppError::ConfigParse {
                    path: directory.to_path_buf(),
                    message: format!(
                        "configuration contains more than {MAX_CONFIG_MODULES} modules"
                    ),
                });
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn merge_config(
    root_path: &Path,
    root: &AppConfig,
    fragments: &[LoadedFragment],
) -> Result<AppConfig, AppError> {
    let mut merged = root.clone();
    let mut page_ids = HashSet::new();
    let mut card_ids = HashSet::new();
    let mut action_ids = HashSet::new();
    register_ids(root_path, "page", &root.pages, &mut page_ids, |page| {
        &page.id
    })?;
    register_ids(root_path, "card", &root.cards, &mut card_ids, |card| {
        &card.id
    })?;
    register_ids(
        root_path,
        "action",
        &root.actions,
        &mut action_ids,
        |action| &action.id,
    )?;

    for fragment in fragments {
        let replaces = fragment.config.replace_existing;
        if fragment.config.app.is_some()
            || fragment.config.ui.is_some()
            || fragment.config.runtime.is_some()
        {
            if !replaces {
                return Err(AppError::ConfigParse {
                    path: fragment.path.clone(),
                    message: "global app/ui/runtime overrides require replace_existing = true"
                        .into(),
                });
            }
            if let Some(app) = &fragment.config.app {
                app.apply_to(&mut merged.app);
            }
            if let Some(ui) = &fragment.config.ui {
                ui.apply_to(&mut merged.ui);
            }
            if let Some(runtime) = &fragment.config.runtime {
                runtime.apply_to(&mut merged.runtime);
            }
        }
        merge_entries(
            &fragment.path,
            "page",
            &fragment.config.pages,
            &mut merged.pages,
            &mut page_ids,
            replaces,
            |page| &page.id,
        )?;
        merge_entries(
            &fragment.path,
            "card",
            &fragment.config.cards,
            &mut merged.cards,
            &mut card_ids,
            replaces,
            |card| &card.id,
        )?;
        merge_entries(
            &fragment.path,
            "action",
            &fragment.config.actions,
            &mut merged.actions,
            &mut action_ids,
            replaces,
            |action| &action.id,
        )?;
    }
    validate_runtime(root_path, &merged)?;
    validate_structure(root_path, &merged)?;
    validate_card_assets(root_path, &merged)?;
    crate::plugins::validate_config(&merged)?;
    Ok(merged)
}

fn validate_runtime(path: &Path, config: &AppConfig) -> Result<(), AppError> {
    for (field, value) in [
        (
            "quiet_hours_start_hour",
            config.runtime.quiet_hours_start_hour,
        ),
        ("quiet_hours_end_hour", config.runtime.quiet_hours_end_hour),
    ] {
        if value > 23 {
            return Err(AppError::ConfigParse {
                path: path.to_path_buf(),
                message: format!("runtime {field} must be between 0 and 23"),
            });
        }
    }
    if !(1..=crate::core::config::HARD_MAX_OBSERVATION_LEASE_SECONDS)
        .contains(&config.runtime.observation_lease_seconds)
    {
        return Err(AppError::ConfigParse {
            path: path.to_path_buf(),
            message: format!(
                "runtime observation_lease_seconds must be between 1 and {}",
                crate::core::config::HARD_MAX_OBSERVATION_LEASE_SECONDS
            ),
        });
    }
    let runtime = &config.runtime;
    let thresholds_valid = runtime.battery_critical_enter_percent
        < runtime.battery_critical_exit_percent
        && runtime.battery_critical_exit_percent <= runtime.battery_low_enter_percent
        && runtime.battery_low_enter_percent < runtime.battery_low_exit_percent;
    if !thresholds_valid || runtime.battery_low_exit_percent > 100 {
        return Err(AppError::ConfigParse {
            path: path.to_path_buf(),
            message: "runtime battery thresholds must satisfy critical_enter < critical_exit <= low_enter < low_exit <= 100".into(),
        });
    }
    for (field, value) in [
        ("inactive_grace_seconds", runtime.inactive_grace_seconds),
        ("idle_timeout_seconds", runtime.idle_timeout_seconds),
        ("idle_stability_seconds", runtime.idle_stability_seconds),
        ("power_sample_seconds", runtime.power_sample_seconds),
        ("thermal_sample_seconds", runtime.thermal_sample_seconds),
        ("agent_attention_seconds", runtime.agent_attention_seconds),
    ] {
        if value > MAX_DURATION_SECONDS {
            return Err(config_error(
                path,
                format!("runtime {field} cannot exceed {MAX_DURATION_SECONDS} seconds"),
            ));
        }
    }
    if runtime.idle_visual_brightness_percent > 100 {
        return Err(config_error(
            path,
            "runtime idle_visual_brightness_percent must be between 0 and 100",
        ));
    }
    Ok(())
}

fn config_error(path: &Path, message: impl Into<String>) -> AppError {
    AppError::ConfigParse {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

fn validate_text(path: &Path, field: &str, value: &str, max: usize) -> Result<(), AppError> {
    if value.trim().is_empty() || value.len() > max {
        return Err(config_error(
            path,
            format!("{field} must be non-empty and at most {max} bytes"),
        ));
    }
    Ok(())
}

fn validate_optional_text(
    path: &Path,
    field: &str,
    value: Option<&String>,
    max: usize,
) -> Result<(), AppError> {
    if let Some(value) = value {
        if value.len() > max {
            return Err(config_error(
                path,
                format!("{field} cannot exceed {max} bytes"),
            ));
        }
    }
    Ok(())
}

fn validate_structure(path: &Path, config: &AppConfig) -> Result<(), AppError> {
    if config.pages.len() > MAX_PAGES {
        return Err(config_error(
            path,
            format!("pages cannot exceed {MAX_PAGES} entries"),
        ));
    }
    if config.cards.len() > MAX_CARDS {
        return Err(config_error(
            path,
            format!("cards cannot exceed {MAX_CARDS} entries"),
        ));
    }
    if config.actions.len() > MAX_ACTIONS {
        return Err(config_error(
            path,
            format!("actions cannot exceed {MAX_ACTIONS} entries"),
        ));
    }
    validate_text(path, "app.title", &config.app.title, 256)?;
    validate_text(path, "app.log_level", &config.app.log_level, 256)?;
    tracing_subscriber::EnvFilter::try_new(&config.app.log_level)
        .map_err(|error| config_error(path, format!("app.log_level is invalid: {error}")))?;
    if !(1..=MAX_OUTPUT_BYTES).contains(&config.app.max_output_bytes) {
        return Err(config_error(
            path,
            format!("app.max_output_bytes must be between 1 and {MAX_OUTPUT_BYTES}"),
        ));
    }
    if !(1..=12).contains(&config.ui.card_columns)
        || config
            .ui
            .card_width
            .is_some_and(|width| !(64..=4096).contains(&width))
        || !(64..=4096).contains(&config.ui.card_height)
    {
        return Err(config_error(
            path,
            "ui card columns/size are outside the supported bounds",
        ));
    }
    validate_text(path, "ui.default_page", &config.ui.default_page, 128)?;

    let page_ids = if config.pages.is_empty() {
        ["monitor", "actions", "settings"]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>()
    } else {
        config.pages.iter().map(|page| page.id.clone()).collect()
    };
    for page in &config.pages {
        validate_text(path, "page.id", &page.id, 128)?;
        validate_text(path, &format!("page {} title", page.id), &page.title, 256)?;
        validate_optional_text(
            path,
            &format!("page {} icon", page.id),
            page.icon.as_ref(),
            256,
        )?;
        validate_optional_text(
            path,
            &format!("page {} kind", page.id),
            page.kind.as_ref(),
            128,
        )?;
    }
    let card_ids = config
        .cards
        .iter()
        .map(|card| card.id.as_str())
        .collect::<HashSet<_>>();
    let action_ids = config
        .actions
        .iter()
        .map(|action| action.id.as_str())
        .collect::<HashSet<_>>();
    if !config.pages.is_empty() && page_ids.len() != config.pages.len() {
        return Err(config_error(path, "page ids must be unique"));
    }
    if action_ids.len() != config.actions.len() {
        return Err(config_error(path, "action ids must be unique"));
    }

    for card in &config.cards {
        validate_card(path, card, &page_ids, &action_ids)?;
    }
    for action in &config.actions {
        validate_action(path, action, &page_ids)?;
    }
    for card in &config.cards {
        if let Some(action) = &card.click_action {
            if !action_ids.contains(action.as_str()) {
                return Err(config_error(
                    path,
                    format!("card {} references unknown action {action}", card.id),
                ));
            }
        }
    }
    // Keep the set materialized above as a cheap duplicate/reference sanity
    // check; merge_config already rejects duplicate ids, while this makes the
    // invariant explicit at the final merged boundary.
    if card_ids.len() != config.cards.len() {
        return Err(config_error(path, "card ids must be unique"));
    }
    Ok(())
}

fn validate_card(
    path: &Path,
    card: &CardConfig,
    page_ids: &HashSet<String>,
    action_ids: &HashSet<&str>,
) -> Result<(), AppError> {
    validate_text(path, "card.id", &card.id, 128)?;
    validate_text(path, &format!("card {} title", card.id), &card.title, 256)?;
    validate_text(path, &format!("card {} page", card.id), &card.page, 128)?;
    if !page_ids.contains(&card.page) {
        return Err(config_error(
            path,
            format!("card {} references unknown page {}", card.id, card.page),
        ));
    }
    if card.refresh_interval == 0 || card.refresh_interval > MAX_DURATION_SECONDS {
        return Err(config_error(
            path,
            format!(
                "card {} refresh must be between 1 and {MAX_DURATION_SECONDS} seconds",
                card.id
            ),
        ));
    }
    if card
        .cache_ttl_seconds
        .is_some_and(|ttl| ttl == 0 || ttl > MAX_DURATION_SECONDS)
    {
        return Err(config_error(
            path,
            format!("card {} cache_ttl is outside the supported bounds", card.id),
        ));
    }
    validate_optional_text(
        path,
        &format!("card {} icon", card.id),
        card.icon.as_ref(),
        256,
    )?;
    validate_optional_text(
        path,
        &format!("card {} description", card.id),
        card.description.as_ref(),
        MAX_TEXT_BYTES,
    )?;
    validate_optional_text(
        path,
        &format!("card {} schedule", card.id),
        card.schedule.as_ref(),
        256,
    )?;
    if let Some(schedule) = card.schedule.as_deref() {
        crate::core::schedule::evaluate(schedule).map_err(|error| {
            config_error(
                path,
                format!("card {} schedule is invalid: {error}", card.id),
            )
        })?;
    }
    if let Some(action) = &card.click_action {
        validate_text(path, &format!("card {} click_action", card.id), action, 128)?;
        if !action_ids.contains(action.as_str()) {
            return Err(config_error(
                path,
                format!("card {} references unknown action {action}", card.id),
            ));
        }
    }
    for (field, value) in [
        (
            "inactive_interval_seconds",
            card.runtime.inactive_interval_seconds,
        ),
        ("idle_interval_seconds", card.runtime.idle_interval_seconds),
        (
            "minimum_interval_seconds",
            card.runtime.minimum_interval_seconds,
        ),
    ] {
        if value.is_some_and(|seconds| seconds == 0 || seconds > MAX_DURATION_SECONDS) {
            return Err(config_error(
                path,
                format!(
                    "card {} runtime {field} is outside the supported bounds",
                    card.id
                ),
            ));
        }
    }
    validate_source(path, card.source.as_ref(), &card.id)?;
    if let Some(display) = &card.display {
        validate_display(path, display, &card.id)?;
    }
    Ok(())
}

fn validate_action(
    path: &Path,
    action: &super::ActionConfig,
    page_ids: &HashSet<String>,
) -> Result<(), AppError> {
    validate_text(path, "action.id", &action.id, 128)?;
    validate_text(
        path,
        &format!("action {} name", action.id),
        &action.name,
        256,
    )?;
    validate_text(
        path,
        &format!("action {} page", action.id),
        &action.page,
        128,
    )?;
    if !page_ids.contains(&action.page) {
        return Err(config_error(
            path,
            format!(
                "action {} references unknown page {}",
                action.id, action.page
            ),
        ));
    }
    validate_optional_text(
        path,
        "action.description",
        action.description.as_ref(),
        MAX_TEXT_BYTES,
    )?;
    validate_optional_text(path, "action.icon", action.icon.as_ref(), 256)?;
    validate_optional_text(
        path,
        "action.confirm_title",
        action.confirm_title.as_ref(),
        256,
    )?;
    validate_optional_text(
        path,
        "action.confirm_detail",
        action.confirm_detail.as_ref(),
        MAX_TEXT_BYTES,
    )?;
    if action.timeout == 0 || action.timeout > MAX_EXTERNAL_TIMEOUT_SECONDS {
        return Err(config_error(
            path,
            format!(
                "action {} timeout must be between 1 and {MAX_EXTERNAL_TIMEOUT_SECONDS} seconds",
                action.id
            ),
        ));
    }
    if action
        .max_output_bytes
        .is_some_and(|value| !(1..=MAX_OUTPUT_BYTES).contains(&value))
    {
        return Err(config_error(
            path,
            format!(
                "action {} max_output_bytes is outside the supported bounds",
                action.id
            ),
        ));
    }
    if let Some(command) = &action.command {
        if command.is_empty()
            || command.len() > 128
            || command
                .iter()
                .any(|part| part.trim().is_empty() || part.len() > 16 * 1024)
        {
            return Err(config_error(
                path,
                format!(
                    "action {} command is outside the supported bounds",
                    action.id
                ),
            ));
        }
    }
    Ok(())
}

fn validate_source(
    path: &Path,
    source: Option<&SourceConfig>,
    card_id: &str,
) -> Result<(), AppError> {
    let Some(source) = source else {
        return Ok(());
    };
    match source {
        SourceConfig::Builtin(metric) => {
            validate_text(path, &format!("card {card_id} builtin"), metric, 128)?;
            if crate::metrics::builtin::create_builtin_metric(metric).is_none() {
                return Err(config_error(
                    path,
                    format!("card {card_id} uses unknown builtin metric {metric}"),
                ));
            }
        }
        SourceConfig::File(file) => {
            validate_text(path, &format!("card {card_id} file.path"), &file.path, 4096)?;
        }
        SourceConfig::Command(command) => {
            if command.run.is_empty()
                || command.run.len() > 128
                || command
                    .run
                    .iter()
                    .any(|part| part.trim().is_empty() || part.len() > 16 * 1024)
            {
                return Err(config_error(
                    path,
                    format!("card {card_id} command.run is invalid"),
                ));
            }
            if command.timeout_seconds == 0
                || command.timeout_seconds > MAX_EXTERNAL_TIMEOUT_SECONDS
            {
                return Err(config_error(
                    path,
                    format!(
                        "card {card_id} command timeout must be between 1 and {MAX_EXTERNAL_TIMEOUT_SECONDS} seconds"
                    ),
                ));
            }
            if !(1..=MAX_OUTPUT_BYTES).contains(&command.max_output_bytes) {
                return Err(config_error(
                    path,
                    format!("card {card_id} command max_output is invalid"),
                ));
            }
            if command.subtitle_lines > 128 {
                return Err(config_error(
                    path,
                    format!("card {card_id} subtitle_lines cannot exceed 128"),
                ));
            }
        }
        SourceConfig::Http(http) => validate_http_source(path, http, card_id)?,
        SourceConfig::Text(value) => {
            if value.len() > MAX_SOURCE_TEXT_BYTES {
                return Err(config_error(
                    path,
                    format!("card {card_id} text source is too large"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_http_source(
    path: &Path,
    http: &HttpSourceConfig,
    card_id: &str,
) -> Result<(), AppError> {
    let url = reqwest::Url::parse(&http.url).map_err(|error| {
        config_error(path, format!("card {card_id} HTTP URL is invalid: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(config_error(
            path,
            format!("card {card_id} HTTP URL must use http or https"),
        ));
    }
    if let Some(method) = &http.method {
        if !matches!(
            method.to_ascii_uppercase().as_str(),
            "GET" | "POST" | "PUT" | "DELETE" | "PATCH"
        ) {
            return Err(config_error(
                path,
                format!("card {card_id} HTTP method is unsupported"),
            ));
        }
    }
    if http.timeout_seconds == 0 || http.timeout_seconds > MAX_EXTERNAL_TIMEOUT_SECONDS {
        return Err(config_error(
            path,
            format!(
                "card {card_id} HTTP timeout must be between 1 and {MAX_EXTERNAL_TIMEOUT_SECONDS} seconds"
            ),
        ));
    }
    if !(1..=MAX_OUTPUT_BYTES).contains(&http.max_output_bytes) {
        return Err(config_error(
            path,
            format!("card {card_id} HTTP max_output is invalid"),
        ));
    }
    if http.headers.as_ref().is_some_and(|headers| {
        headers.len() > 64
            || headers
                .iter()
                .any(|(key, value)| key.len() > 256 || value.len() > 4096)
    }) {
        return Err(config_error(
            path,
            format!("card {card_id} HTTP headers are too large"),
        ));
    }
    if http
        .body
        .as_ref()
        .is_some_and(|body| body.len() > MAX_SOURCE_TEXT_BYTES)
    {
        return Err(config_error(
            path,
            format!("card {card_id} HTTP body is too large"),
        ));
    }
    if let Some(parser) = &http.parser {
        validate_parser(path, parser, card_id)?;
    }
    Ok(())
}

fn validate_parser(path: &Path, parser: &ParserConfig, card_id: &str) -> Result<(), AppError> {
    if parser
        .divisor
        .is_some_and(|value| !value.is_finite() || value == 0.0)
        || parser.multiplier.is_some_and(|value| !value.is_finite())
    {
        return Err(config_error(
            path,
            format!("card {card_id} parser scale must be finite"),
        ));
    }
    if parser.decimal_places.is_some_and(|value| value > 12) {
        return Err(config_error(
            path,
            format!("card {card_id} parser decimal_places cannot exceed 12"),
        ));
    }
    if parser.capture.is_some_and(|value| value > 64) {
        return Err(config_error(
            path,
            format!("card {card_id} parser capture cannot exceed 64"),
        ));
    }
    validate_optional_text(path, "parser.pattern", parser.pattern.as_ref(), 16 * 1024)?;
    validate_optional_text(path, "parser.path", parser.path.as_ref(), 1024)?;
    validate_optional_text(path, "parser.suffix", parser.suffix.as_ref(), 1024)?;
    if parser.parser_type == ParserKind::Regex {
        let Some(pattern) = &parser.pattern else {
            return Err(config_error(
                path,
                format!("card {card_id} regex parser requires pattern"),
            ));
        };
        regex::RegexBuilder::new(pattern)
            .size_limit(1024 * 1024)
            .build()
            .map_err(|error| {
                config_error(path, format!("card {card_id} regex is invalid: {error}"))
            })?;
    }
    Ok(())
}

fn validate_display(path: &Path, display: &DisplayConfig, card_id: &str) -> Result<(), AppError> {
    if display
        .minimum_change
        .is_some_and(|value| !value.is_finite() || value < 0.0)
        || display.columns_after.is_some_and(|value| value > 512)
        || display
            .columns
            .is_some_and(|value| !(1..=64).contains(&value))
        || display
            .card_width
            .is_some_and(|value| !(64..=4096).contains(&value))
        || display
            .card_height
            .is_some_and(|value| !(64..=4096).contains(&value))
    {
        return Err(config_error(
            path,
            format!("card {card_id} display bounds are invalid"),
        ));
    }
    if display
        .transition
        .as_ref()
        .is_some_and(|transition| transition.duration_ms > 2000)
    {
        return Err(config_error(
            path,
            format!("card {card_id} transition duration is too large"),
        ));
    }
    validate_transition(path, display.transition.as_ref(), card_id)?;
    validate_colors(path, &display.colors, card_id)?;
    if display.states.len() > 64 {
        return Err(config_error(
            path,
            format!("card {card_id} cannot contain more than 64 visual states"),
        ));
    }
    for state in &display.states {
        validate_visual_state(path, state, card_id)?;
    }
    Ok(())
}

fn validate_transition(
    path: &Path,
    transition: Option<&CardTransitionConfig>,
    card_id: &str,
) -> Result<(), AppError> {
    if let Some(transition) = transition {
        validate_text(
            path,
            &format!("card {card_id} transition.easing"),
            &transition.easing,
            64,
        )?;
    }
    Ok(())
}

fn validate_colors(
    path: &Path,
    colors: &super::CardColorsConfig,
    card_id: &str,
) -> Result<(), AppError> {
    let values = [
        colors.accent.as_ref(),
        colors.value.as_ref(),
        colors.title.as_ref(),
        colors.icon.as_ref(),
        colors.subtitle.as_ref(),
        colors.footer.as_ref(),
        colors.progress.as_ref(),
    ];
    if values
        .iter()
        .flatten()
        .any(|value| value.trim().is_empty() || value.len() > 128)
        || colors.background.len() > 4
        || colors
            .background
            .iter()
            .any(|value| value.trim().is_empty() || value.len() > 128)
        || colors
            .background_opacity
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(config_error(
            path,
            format!("card {card_id} colors are invalid"),
        ));
    }
    Ok(())
}

fn validate_visual_state(
    path: &Path,
    state: &CardVisualStateConfig,
    card_id: &str,
) -> Result<(), AppError> {
    validate_text(
        path,
        &format!("card {card_id} visual state name"),
        &state.name,
        128,
    )?;
    if state.min.is_some_and(|value| !value.is_finite())
        || state.max.is_some_and(|value| !value.is_finite())
        || state.min.zip(state.max).is_some_and(|(min, max)| min > max)
    {
        return Err(config_error(
            path,
            format!("card {card_id} visual state bounds are invalid"),
        ));
    }
    validate_optional_text(
        path,
        "visual state equals",
        state.equals.as_ref(),
        MAX_TEXT_BYTES,
    )?;
    validate_optional_text(
        path,
        "visual state contains",
        state.contains.as_ref(),
        MAX_TEXT_BYTES,
    )?;
    validate_optional_text(path, "visual state regex", state.regex.as_ref(), 16 * 1024)?;
    validate_optional_text(
        path,
        "visual state label",
        state.label.as_ref(),
        MAX_TEXT_BYTES,
    )?;
    if let Some(pattern) = &state.regex {
        regex::RegexBuilder::new(pattern)
            .size_limit(1024 * 1024)
            .case_insensitive(state.ignore_case)
            .build()
            .map_err(|error| {
                config_error(
                    path,
                    format!("card {card_id} visual state regex is invalid: {error}"),
                )
            })?;
    }
    validate_colors(path, &state.colors, card_id)
}

fn validate_card_assets(path: &Path, config: &AppConfig) -> Result<(), AppError> {
    let config_directory = path.parent().unwrap_or_else(|| Path::new("."));
    for card in &config.cards {
        if let Some(background) = card
            .display
            .as_ref()
            .and_then(|display| display.background_svg.as_ref())
        {
            if !background.opacity.is_finite() || !(0.0..=1.0).contains(&background.opacity) {
                return Err(AppError::ConfigParse {
                    path: path.to_path_buf(),
                    message: format!(
                        "card {} background_svg opacity must be between 0.0 and 1.0",
                        card.id
                    ),
                });
            }
            background
                .resolve_path(config_directory)
                .map_err(|message| AppError::ConfigParse {
                    path: path.to_path_buf(),
                    message: format!("card {} background_svg {message}", card.id),
                })?;
        }
        if let Some(logo) = card
            .display
            .as_ref()
            .and_then(|display| display.logo_svg.as_ref())
        {
            if !(8..=64).contains(&logo.size) {
                return Err(AppError::ConfigParse {
                    path: path.to_path_buf(),
                    message: format!("card {} logo_svg size must be between 8 and 64", card.id),
                });
            }
            if !logo.opacity.is_finite() || !(0.0..=1.0).contains(&logo.opacity) {
                return Err(AppError::ConfigParse {
                    path: path.to_path_buf(),
                    message: format!(
                        "card {} logo_svg opacity must be between 0.0 and 1.0",
                        card.id
                    ),
                });
            }
            logo.resolve_path(config_directory)
                .map_err(|message| AppError::ConfigParse {
                    path: path.to_path_buf(),
                    message: format!("card {} logo_svg {message}", card.id),
                })?;
        }
    }
    Ok(())
}

fn merge_entries<T: Clone>(
    path: &Path,
    kind: &str,
    incoming: &[T],
    merged: &mut Vec<T>,
    known: &mut HashSet<String>,
    replace_existing: bool,
    id: fn(&T) -> &str,
) -> Result<(), AppError> {
    let mut local = HashSet::new();
    for entry in incoming {
        let value = id(entry);
        if value.trim().is_empty() {
            return Err(AppError::ConfigParse {
                path: path.to_path_buf(),
                message: format!("{kind} id cannot be empty"),
            });
        }
        if !local.insert(value.to_string()) {
            return Err(AppError::ConfigParse {
                path: path.to_path_buf(),
                message: format!("duplicate {kind} id in the same module: {value}"),
            });
        }
        if known.contains(value) {
            if !replace_existing {
                return Err(AppError::ConfigParse {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate {kind} id: {value}; set replace_existing = true for an intentional override"
                    ),
                });
            }
            let current = merged
                .iter_mut()
                .find(|current| id(current) == value)
                .expect("known config entry exists in merged config");
            *current = entry.clone();
        } else {
            known.insert(value.to_string());
            merged.push(entry.clone());
        }
    }
    Ok(())
}

fn register_ids<T>(
    path: &Path,
    kind: &str,
    entries: &[T],
    ids: &mut HashSet<String>,
    id: fn(&T) -> &str,
) -> Result<(), AppError> {
    for entry in entries {
        let value = id(entry);
        if value.trim().is_empty() {
            return Err(AppError::ConfigParse {
                path: path.to_path_buf(),
                message: format!("{kind} id cannot be empty"),
            });
        }
        if !ids.insert(value.to_string()) {
            return Err(AppError::ConfigParse {
                path: path.to_path_buf(),
                message: format!("duplicate {kind} id: {value}"),
            });
        }
    }
    Ok(())
}

fn sync_owned_entries<T: Clone>(
    target: &mut [T],
    merged: &[T],
    owners: &HashMap<String, usize>,
    owner: Option<usize>,
    id: fn(&T) -> &str,
) {
    for entry in target {
        if owners.get(id(entry)).copied() != owner {
            continue;
        }
        if let Some(current) = merged.iter().find(|current| id(current) == id(entry)) {
            *entry = current.clone();
        }
    }
}

fn entry_ids<T>(entries: &[T], id: fn(&T) -> &str) -> HashSet<String> {
    entries.iter().map(|entry| id(entry).to_string()).collect()
}

fn serialize_document<T: Serialize>(path: &Path, value: &T) -> Result<String, AppError> {
    if path.extension().and_then(|value| value.to_str()) == Some("json") {
        serde_json::to_string_pretty(value).map_err(|error| AppError::Config(error.to_string()))
    } else {
        toml::to_string_pretty(value).map_err(|error| AppError::Config(error.to_string()))
    }
}

fn write_if_changed<T: Serialize>(path: &Path, previous: &T, next: &T) -> Result<(), AppError> {
    if serialize_document(path, previous)? != serialize_document(path, next)? {
        write_document(path, next)?;
    }
    Ok(())
}

fn write_document<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    let content = serialize_document(path, value)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("config");
    let temporary = path.with_file_name(format!(".{file_name}.tmp"));
    std::fs::write(&temporary, content)?;
    #[cfg(unix)]
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(temporary, path)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}
