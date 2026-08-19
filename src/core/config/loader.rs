use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::{AppConfig, ConfigFragment, CONFIG_SCHEMA_VERSION};
use crate::core::error::AppError;

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

        let merged = merge_config(&self.path, &self.root, &fragments)?;
        let next = fragments
            .iter()
            .find(|fragment| fragment.path == path)
            .expect("personal module is present");
        std::fs::create_dir_all(self.module_dir())?;
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
        std::fs::create_dir_all(self.module_dir())?;
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
    let content = std::fs::read_to_string(path).map_err(|error| AppError::ConfigParse {
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
    Ok(merged)
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
    std::fs::rename(temporary, path)?;
    Ok(())
}
