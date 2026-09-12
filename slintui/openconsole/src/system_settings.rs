use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

pub const DEFAULT_SYSTEM_SETTINGS_CONFIG_PATH: &str = "/data/openconsole/system-settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSettings {
    #[serde(default)]
    pub wifi_enabled: bool,
    #[serde(default)]
    pub bluetooth_enabled: bool,
    #[serde(default)]
    pub preferred_wifi_network: Option<String>,
}

impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            wifi_enabled: false,
            bluetooth_enabled: false,
            preferred_wifi_network: None,
        }
    }
}

pub fn system_settings_config_path() -> PathBuf {
    std::env::var("OPENCONSOLE_SYSTEM_SETTINGS_JSON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SYSTEM_SETTINGS_CONFIG_PATH))
}

pub fn load_system_settings() -> Result<SystemSettings, String> {
    let path = system_settings_config_path();
    if !path.exists() {
        return Ok(SystemSettings::default());
    }

    let content = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read system settings {}: {error}", path.display()))?;

    serde_json::from_str(&content).map_err(|error| {
        format!(
            "failed to parse system settings {}: {error}",
            path.display()
        )
    })
}

pub fn load_or_create_system_settings() -> Result<SystemSettings, String> {
    let path = system_settings_config_path();
    let settings = load_system_settings()?;

    if !path.exists() {
        save_system_settings(&settings)?;
    }

    Ok(settings)
}

pub fn save_system_settings(settings: &SystemSettings) -> Result<(), String> {
    let path = system_settings_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create system settings directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let payload = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("failed to serialize system settings: {error}"))?;

    fs::write(&path, payload).map_err(|error| {
        format!(
            "failed to write system settings {}: {error}",
            path.display()
        )
    })
}

pub fn apply_system_settings(settings: &SystemSettings) -> Result<(), String> {
    apply_wifi(settings.wifi_enabled, settings.preferred_wifi_network.as_deref())?;
    apply_bluetooth(settings.bluetooth_enabled)
}

fn apply_wifi(enabled: bool, preferred_wifi_network: Option<&str>) -> Result<(), String> {
    let command = if enabled {
        "systemctl enable --now iwd.service || true; systemctl disable --now wpa_supplicant.service || true"
    } else {
        "systemctl disable --now iwd.service || true; systemctl disable --now wpa_supplicant.service || true"
    };

    let _ = preferred_wifi_network;
    run_shell_command("wifi", command)
}

fn apply_bluetooth(enabled: bool) -> Result<(), String> {
    let command = if enabled {
        "systemctl enable --now bluetooth.service || true; bluetoothctl power on || true"
    } else {
        "bluetoothctl power off || true; systemctl disable --now bluetooth.service || true"
    };

    run_shell_command("bluetooth", command)
}

fn run_shell_command(label: &str, command: &str) -> Result<(), String> {
    let status = Command::new("sh")
        .arg("-lc")
        .arg(command)
        .status()
        .map_err(|error| format!("failed to run {label} command: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} command failed with status {status}"))
    }
}
