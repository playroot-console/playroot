use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

pub const DEFAULT_CONTROLLER_CONFIG_PATH: &str = "/data/openconsole/controllers.json";
static CONTROLLER_STORE_REVISION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ControllerStore {
    #[serde(default)]
    pub controllers: Vec<ControllerProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerProfile {
    pub id: String,
    pub name: String,
    pub template: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub bluetooth_address: Option<String>,
    #[serde(default)]
    pub mappings: BTreeMap<String, String>,
}

pub fn controller_config_path() -> PathBuf {
    std::env::var("OPENCONSOLE_CONTROLLERS_JSON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CONTROLLER_CONFIG_PATH))
}

pub fn load_store() -> Result<ControllerStore, String> {
    let path = controller_config_path();
    if !path.exists() {
        return Ok(ControllerStore::default());
    }

    let content = fs::read_to_string(&path).map_err(|error| {
        format!(
            "failed to read controller config {}: {error}",
            path.display()
        )
    })?;

    if content.trim().is_empty() {
        return Ok(ControllerStore::default());
    }

    serde_json::from_str(&content).map_err(|error| {
        format!(
            "failed to parse controller config {}: {error}",
            path.display()
        )
    })
}

pub fn save_store(store: &ControllerStore) -> Result<(), String> {
    let path = controller_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create controller config directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let payload = serde_json::to_string_pretty(store)
        .map_err(|error| format!("failed to serialize controller config: {error}"))?;

    fs::write(&path, payload).map_err(|error| {
        format!(
            "failed to write controller config {}: {error}",
            path.display()
        )
    })?;

    CONTROLLER_STORE_REVISION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub fn controller_store_revision() -> u64 {
    CONTROLLER_STORE_REVISION.load(Ordering::Relaxed)
}

pub fn list_text(store: &ControllerStore) -> String {
    list_text_with_selection(store, None)
}

pub fn list_text_with_selection(store: &ControllerStore, selected_index: Option<usize>) -> String {
    if store.controllers.is_empty() {
        return "No controllers added yet.".to_string();
    }

    store
        .controllers
        .iter()
        .enumerate()
        .map(|(idx, controller)| {
            let marker = if Some(idx) == selected_index {
                ">"
            } else {
                " "
            };
            format!(
                "{} {}. {} ({})",
                marker,
                idx + 1,
                controller.name,
                controller.template
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
