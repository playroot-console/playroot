use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::logging::log_launch;

#[cfg(target_os = "linux")]
const PROC_INPUT_DEVICES_PATH: &str = "/proc/bus/input/devices";
#[cfg(target_os = "linux")]
const SYS_CLASS_INPUT_PATH: &str = "/sys/class/input";
#[cfg(target_os = "linux")]
const SYS_BUS_USB_DEVICES_PATH: &str = "/sys/bus/usb/devices";
#[cfg(target_os = "linux")]
const HUB_ENABLE_DELAY: Duration = Duration::from_millis(200);
#[cfg(target_os = "linux")]
const DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(500);
#[cfg(target_os = "linux")]
const DISCOVERY_RETRY_ATTEMPTS: usize = 12;
#[cfg(target_os = "linux")]
const DEFAULT_FALLBACK_HUB_LOCATIONS: &[&str] = &["1-1"];

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct UsbGamepadTarget {
    event_node: String,
    device_name: String,
    usb_device_path: PathBuf,
    hub_location: String,
}

#[cfg(target_os = "linux")]
pub fn recover_usb_gamepads_after_boot() {
    let targets = discover_usb_gamepad_targets_with_retry();
    let mut seen_hubs = HashSet::new();
    let mut hubs_to_cycle = Vec::new();

    for location in fallback_hub_locations() {
        if seen_hubs.insert(location.clone()) {
            hubs_to_cycle.push((
                location,
                None::<(&str, &str, &Path)>,
            ));
        }
    }

    for target in &targets {
        if seen_hubs.insert(target.hub_location.clone()) {
            hubs_to_cycle.push((
                target.hub_location.clone(),
                Some((
                    target.device_name.as_str(),
                    target.event_node.as_str(),
                    target.usb_device_path.as_path(),
                )),
            ));
        }
    }

    if hubs_to_cycle.is_empty() {
        log_launch("usb gamepad recovery: no USB gamepads detected and no fallback hubs configured");
        return;
    }

    let mut reset_count = 0usize;

    for (hub_location, target_details) in hubs_to_cycle {
        match target_details {
            Some((device_name, event_node, _)) => log_launch(&format!(
                "usb gamepad recovery: power cycling hub {} for {} ({})",
                hub_location, device_name, event_node
            )),
            None => log_launch(&format!(
                "usb gamepad recovery: power cycling fallback hub {} before controller enumeration",
                hub_location
            )),
        }

        match power_cycle_usb_hub(&hub_location) {
            Ok(()) => {
                reset_count += 1;
                match target_details {
                    Some((device_name, _, _)) => log_launch(&format!(
                        "usb gamepad recovery: power cycled hub {} for {}",
                        hub_location, device_name
                    )),
                    None => log_launch(&format!(
                        "usb gamepad recovery: power cycled fallback hub {}",
                        hub_location
                    )),
                }
            }
            Err(error) => {
                if let Some((device_name, _, usb_device_path)) = target_details {
                    log_launch(&format!(
                        "usb gamepad recovery: uhubctl failed for hub {} ({}) - falling back to authorized toggle: {error}",
                        hub_location, device_name
                    ));

                    if let Err(fallback_error) = toggle_authorized(usb_device_path) {
                        log_launch(&format!(
                            "usb gamepad recovery: authorized toggle failed for {}: {fallback_error}",
                            usb_device_path.display()
                        ));
                    } else {
                        reset_count += 1;
                    }
                } else {
                    log_launch(&format!(
                        "usb gamepad recovery: uhubctl failed for fallback hub {}: {error}",
                        hub_location
                    ));
                }
            }
        }
    }

    if reset_count == 0 {
        log_launch("usb gamepad recovery: no USB hubs or devices were updated");
    }
}

#[cfg(target_os = "linux")]
fn fallback_hub_locations() -> Vec<String> {
    let configured = std::env::var("OPENCONSOLE_USB_GAMEPAD_HUBS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if configured.is_empty() {
        DEFAULT_FALLBACK_HUB_LOCATIONS
            .iter()
            .map(|value| value.to_string())
            .collect()
    } else {
        configured
    }
}

#[cfg(not(target_os = "linux"))]
pub fn recover_usb_gamepads_after_boot() {}

#[cfg(target_os = "linux")]
fn discover_usb_gamepad_targets_with_retry() -> Vec<UsbGamepadTarget> {
    for attempt in 0..DISCOVERY_RETRY_ATTEMPTS {
        let targets = discover_usb_gamepad_targets();
        if !targets.is_empty() {
            if attempt > 0 {
                log_launch(&format!(
                    "usb gamepad recovery: detected {} USB gamepad(s) after {} retry attempt(s)",
                    targets.len(),
                    attempt
                ));
            }
            return targets;
        }

        if attempt == 0 {
            log_launch("usb gamepad recovery: no USB gamepads detected yet; waiting for enumeration");
        }

        if attempt + 1 < DISCOVERY_RETRY_ATTEMPTS {
            thread::sleep(DISCOVERY_RETRY_DELAY);
        }
    }

    Vec::new()
}

#[cfg(target_os = "linux")]
fn discover_usb_gamepad_targets() -> Vec<UsbGamepadTarget> {
    let Ok(contents) = fs::read_to_string(PROC_INPUT_DEVICES_PATH) else {
        return Vec::new();
    };

    let mut targets = Vec::new();
    for block in contents.split("\n\n") {
        let Some(handlers) = parse_proc_bus_value(block, "H") else {
            continue;
        };

        let Some(event_node) = handlers
            .split_whitespace()
            .find(|handler| handler.starts_with("event"))
            .map(str::to_string)
        else {
            continue;
        };

        if !looks_like_usb_gamepad(block, &handlers) {
            continue;
        }

        let device_name = parse_proc_bus_name(block).unwrap_or_else(|| event_node.clone());
        let Some((usb_device_path, hub_location)) = resolve_usb_device_path(&event_node) else {
            continue;
        };

        targets.push(UsbGamepadTarget {
            event_node,
            device_name,
            usb_device_path,
            hub_location,
        });
    }

    targets
}

#[cfg(target_os = "linux")]
fn looks_like_usb_gamepad(block: &str, handlers: &str) -> bool {
    let lower_block = block.to_lowercase();
    let lower_handlers = handlers.to_lowercase();

    if lower_block.contains("keyboard") || lower_block.contains("mouse") {
        return false;
    }

    if lower_handlers.split_whitespace().any(|handler| handler.starts_with("js")) {
        return true;
    }

    [
        "controller",
        "gamepad",
        "dualshock",
        "dualsense",
        "xbox",
        "switch",
        "joy-con",
        "pro controller",
        "wireless controller",
        "wired controller",
        "8bitdo",
    ]
    .iter()
    .any(|needle| lower_block.contains(needle))
}

#[cfg(target_os = "linux")]
fn resolve_usb_device_path(event_node: &str) -> Option<(PathBuf, String)> {
    let event_device_path = Path::new(SYS_CLASS_INPUT_PATH).join(event_node).join("device");
    let resolved_path = fs::canonicalize(event_device_path).ok()?;

    for ancestor in resolved_path.ancestors() {
        let Some(name) = ancestor.file_name().and_then(|value| value.to_str()) else {
            continue;
        };

        if !name.contains('-') {
            continue;
        }

        let base_name = name.split(':').next().unwrap_or(name);
        let usb_device_path = Path::new(SYS_BUS_USB_DEVICES_PATH).join(base_name);
        if !usb_device_path.exists() {
            continue;
        }

        let hub_location = usb_hub_location(base_name);
        return Some((usb_device_path, hub_location));
    }

    None
}

#[cfg(target_os = "linux")]
fn usb_hub_location(device_name: &str) -> String {
    match device_name.rsplit_once('.') {
        Some((hub, _)) if !hub.is_empty() => hub.to_string(),
        _ => device_name.to_string(),
    }
}

#[cfg(target_os = "linux")]
fn power_cycle_usb_hub(location: &str) -> Result<(), String> {
    run_uhubctl(location, "0")?;
    thread::sleep(HUB_ENABLE_DELAY);
    run_uhubctl(location, "1")
}

#[cfg(target_os = "linux")]
fn run_uhubctl(location: &str, action: &str) -> Result<(), String> {
    let status = Command::new("uhubctl")
        .args(["-l", location, "-a", action])
        .status()
        .map_err(|error| format!("failed to run uhubctl -a {action}: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("uhubctl -a {action} exited with status {status}"))
    }
}

#[cfg(target_os = "linux")]
fn toggle_authorized(usb_device_path: &Path) -> Result<(), String> {
    let authorized_path = usb_device_path.join("authorized");
    if !authorized_path.exists() {
        return Err(format!(
            "authorized file not found at {}",
            authorized_path.display()
        ));
    }

    fs::write(&authorized_path, b"0\n")
        .map_err(|error| format!("failed to deauthorize {}: {error}", authorized_path.display()))?;
    thread::sleep(HUB_ENABLE_DELAY);
    fs::write(&authorized_path, b"1\n")
        .map_err(|error| format!("failed to reauthorize {}: {error}", authorized_path.display()))
}

#[cfg(target_os = "linux")]
fn parse_proc_bus_value<'a>(block: &'a str, prefix: &str) -> Option<&'a str> {
    block.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(':'))
            .map(str::trim)
    })
}

#[cfg(target_os = "linux")]
fn parse_proc_bus_name(block: &str) -> Option<String> {
    parse_proc_bus_value(block, "N").and_then(|value| {
        value
            .strip_prefix("Name=")
            .map(|name| name.trim_matches('"').to_string())
    })
}
