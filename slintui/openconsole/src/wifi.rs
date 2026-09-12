use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const IWCTL_TIMEOUT: Duration = Duration::from_secs(8);
const IWD_READY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IwdServiceState {
    Active,
    Activating,
    Inactive,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Default)]
pub struct WifiNetwork {
    pub ssid: String,
    pub secured: bool,
    pub known: bool,
    pub connected: bool,
    pub signal: String,
}

#[derive(Debug, Clone, Default)]
pub struct WifiStatus {
    pub interface_name: Option<String>,
    pub ethernet_interface: Option<String>,
    pub ethernet_active: bool,
    pub ethernet_ipv4_addresses: Vec<String>,
    pub ethernet_ipv6_addresses: Vec<String>,
    pub connected_ssid: Option<String>,
    pub saved_ssid: Option<String>,
    pub ipv4_addresses: Vec<String>,
    pub ipv6_addresses: Vec<String>,
    pub security: Option<String>,
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
struct WifiConnectionInfo {
    ssid: Option<String>,
    security: Option<String>,
    ipv4_addresses: Vec<String>,
    ipv6_addresses: Vec<String>,
}

pub fn read_status(enabled: bool, saved_ssid: Option<&str>) -> WifiStatus {
    let interface_name = wifi_interface();
    let ethernet_interface = active_ethernet_interface();
    let ethernet_active = ethernet_interface.is_some();
    let ethernet_ipv4_addresses = ethernet_interface
        .as_deref()
        .map(|iface| ip_addresses(iface, false))
        .unwrap_or_default();
    let ethernet_ipv6_addresses = ethernet_interface
        .as_deref()
        .map(|iface| ip_addresses(iface, true))
        .unwrap_or_default();
    let iwd_state = iwd_service_state();
    let iwd_active = iwd_state == IwdServiceState::Active;
    let connection_info = if enabled && iwd_active {
        interface_name
            .as_deref()
            .map(read_connection_info)
            .unwrap_or_default()
    } else {
        WifiConnectionInfo::default()
    };
    let connected_ssid = connection_info.ssid.clone();
    let saved_ssid = saved_ssid
        .map(str::trim)
        .filter(|ssid| !ssid.is_empty())
        .map(str::to_string);

    let (title, detail) = if !enabled {
        (
            "Disabled".to_string(),
            "Enable Wi-Fi to scan and connect to a network.".to_string(),
        )
    } else if interface_name.is_none() {
        (
            "Unavailable".to_string(),
            "No wireless interface was detected.".to_string(),
        )
    } else if iwd_state == IwdServiceState::Failed {
        (
            "Failed".to_string(),
            "Wi-Fi service failed to start. Check iwd.service on the system image.".to_string(),
        )
    } else if matches!(iwd_state, IwdServiceState::Inactive | IwdServiceState::Unknown) {
        (
            "Offline".to_string(),
            "Wi-Fi service is not running.".to_string(),
        )
    } else if !iwd_active {
        (
            saved_ssid
                .clone()
                .unwrap_or_else(|| "Starting...".to_string()),
            "Wi-Fi service is still starting.".to_string(),
        )
    } else if ethernet_active {
        let title = connected_ssid
            .clone()
            .or_else(|| saved_ssid.clone())
            .unwrap_or_else(|| "Ethernet active".to_string());
        (
            title,
            format!(
                "Ethernet is active on {}. Wi-Fi is on standby.",
                ethernet_interface.clone().unwrap_or_else(|| "wired".to_string())
            ),
        )
    } else if let Some(ssid) = connected_ssid.clone() {
        let mut detail = "Connected over Wi-Fi.".to_string();
        if !connection_info.ipv4_addresses.is_empty() {
            detail.push_str(&format!(" IPv4: {}.", connection_info.ipv4_addresses.join(", ")));
        } else {
            detail.push_str(" No IPv4 address assigned.");
        }
        if !connection_info.ipv6_addresses.is_empty() {
            detail.push_str(&format!(" IPv6: {}.", connection_info.ipv6_addresses.join(", ")));
        }
        if let Some(security) = connection_info.security.clone() {
            detail.push_str(&format!(" Security: {security}."));
        }
        (ssid, detail)
    } else if let Some(ssid) = saved_ssid.clone() {
        (
            ssid,
            "Trying saved Wi-Fi network when available.".to_string(),
        )
    } else {
        (
            "Setup needed".to_string(),
            "Select a network and save its settings.".to_string(),
        )
    };

    WifiStatus {
        interface_name,
        ethernet_interface,
        ethernet_active,
        ethernet_ipv4_addresses,
        ethernet_ipv6_addresses,
        connected_ssid,
        saved_ssid,
        ipv4_addresses: connection_info.ipv4_addresses,
        ipv6_addresses: connection_info.ipv6_addresses,
        security: connection_info.security,
        title,
        detail,
    }
}

pub fn refresh_network_status(networks: &mut [WifiNetwork]) {
    let connected_ssid = wifi_interface()
        .as_deref()
        .and_then(current_connected_network);
    let known_networks = known_network_names().unwrap_or_default();

    for network in networks.iter_mut() {
        network.connected = connected_ssid.as_deref() == Some(network.ssid.as_str());
        network.known = known_networks.iter().any(|known| known == &network.ssid);
    }
}

pub fn enforce_connectivity_policy(
    enabled: bool,
    saved_ssid: Option<&str>,
) -> Result<WifiStatus, String> {
    if !enabled {
        if iwd_service_state() == IwdServiceState::Active {
            let _ = disconnect_wifi();
        }
        return Ok(read_status(false, saved_ssid));
    }

    let Some(interface_name) = wifi_interface() else {
        return Ok(read_status(true, saved_ssid));
    };

    if wait_for_iwd_ready(IWD_READY_TIMEOUT).is_err() {
        return Ok(read_status(true, saved_ssid));
    }

    if active_ethernet_interface().is_some() {
        let _ = disconnect_wifi();
        return Ok(read_status(true, saved_ssid));
    }

    let current_ssid = current_connected_network(&interface_name);
    let saved_ssid = saved_ssid.map(str::trim).filter(|ssid| !ssid.is_empty());

    if current_ssid.is_none() {
        if let Some(ssid) = saved_ssid {
            if known_network_names()?.iter().any(|known| known == ssid) {
                let _ = connect_to_network(ssid, None);
                thread::sleep(Duration::from_secs(1));
            }
        }
    }

    Ok(read_status(true, saved_ssid))
}

pub fn scan_networks() -> Result<Vec<WifiNetwork>, String> {
    let Some(interface_name) = wifi_interface() else {
        return Err("no wireless interface was detected".to_string());
    };

    match wait_for_iwd_ready(IWD_READY_TIMEOUT) {
        Ok(()) => {}
        Err(error) => return Err(error),
    }

    let _ = run_iwctl(["station", interface_name.as_str(), "scan"])?;
    thread::sleep(Duration::from_secs(2));

    let known = known_network_names()?;
    let output = run_iwctl(["station", interface_name.as_str(), "get-networks"])?;
    let mut networks = Vec::new();
    let mut in_table = false;

    for raw_line in output.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }

        if !in_table {
            if line.contains("Available networks") {
                in_table = true;
            }
            continue;
        }

        if line.trim_start().starts_with('-') || line.contains("Network name") {
            continue;
        }

        let connected = line.trim_start().starts_with('>');
        let cleaned = line
            .trim_start_matches(|ch: char| ch == '>' || ch == '*' || ch.is_whitespace())
            .trim();
        if cleaned.is_empty() {
            continue;
        }

        let columns = split_columns(cleaned);
        if columns.is_empty() {
            continue;
        }

        let ssid = columns[0].trim().to_string();
        if ssid.is_empty() {
            continue;
        }

        let security = columns.get(1).cloned().unwrap_or_default();
        let signal = columns.get(2).cloned().unwrap_or_default();
        let security_lower = security.to_ascii_lowercase();
        let secured = !(security_lower.is_empty()
            || security_lower == "open"
            || security_lower == "none");

        networks.push(WifiNetwork {
            known: known.iter().any(|known_ssid| known_ssid == &ssid),
            ssid,
            secured,
            connected,
            signal,
        });
    }

    Ok(networks)
}

pub fn connect_to_network(ssid: &str, passphrase: Option<&str>) -> Result<(), String> {
    let Some(interface_name) = wifi_interface() else {
        return Err("no wireless interface was detected".to_string());
    };

    if let Err(error) = wait_for_iwd_ready(IWD_READY_TIMEOUT) {
        return Err(error);
    }

    let output = if let Some(passphrase) = passphrase {
        Command::new("iwctl")
            .arg("--passphrase")
            .arg(passphrase)
            .arg("station")
            .arg(&interface_name)
            .arg("connect")
            .arg(ssid)
            .output()
            .map_err(|error| format!("failed to run iwctl connect: {error}"))?
    } else {
        Command::new("iwctl")
            .arg("station")
            .arg(&interface_name)
            .arg("connect")
            .arg(ssid)
            .output()
            .map_err(|error| format!("failed to run iwctl connect: {error}"))?
    };

    if output.status.success() {
        Ok(())
    } else {
        let stderr = sanitize_command_output(&String::from_utf8_lossy(&output.stderr));
        let stdout = sanitize_command_output(&String::from_utf8_lossy(&output.stdout));
        let details = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("iwctl connect failed with status {}", output.status)
        };

        Err(describe_connect_failure(ssid, &details))
    }
}

fn describe_connect_failure(ssid: &str, details: &str) -> String {
    let recent_logs = recent_iwd_logs();
    let mut combined = details.to_string();
    if let Some(logs) = recent_logs.as_deref() {
        if !logs.is_empty() {
            combined.push('\n');
            combined.push_str(logs);
        }
    }

    let combined_lower = combined.to_ascii_lowercase();
    let details_lower = details.to_ascii_lowercase();
    let saw_sae = combined_lower.contains("sae unsupported")
        || combined_lower.contains("cmd_external_auth for sae");
    let saw_reason_15 = combined_lower.contains("reason: 15");

    if saw_sae && saw_reason_15 {
        return format!(
            "{ssid} failed during authentication. The access point advertised an SAE/WPA3 path during negotiation, and this Wi-Fi adapter/driver cannot use SAE reliably. On mixed WPA2-PSK/WPA3-SAE networks this can still happen even when WPA2 is enabled. Also verify the passphrase and make sure the access point changes were fully applied."
        );
    }

    if saw_sae {
        return format!(
            "{ssid} advertised an SAE/WPA3 authentication path during negotiation, and this Wi-Fi adapter/driver does not support SAE reliably. If this network is meant to allow WPA2-PSK clients, verify that the access point changes were fully applied and retry."
        );
    }

    if saw_reason_15 {
        return format!(
            "{ssid} rejected the connection during authentication or key exchange. Verify the passphrase and access point security settings."
        );
    }

    if details_lower.contains("operation aborted") {
        return format!(
            "{ssid} connection was aborted before completion. Retry once more and check recent iwd.service logs on the device for the exact authentication reason."
        );
    }

    format!("{details}")
}

fn recent_iwd_logs() -> Option<String> {
    let output = Command::new("journalctl")
        .arg("-u")
        .arg("iwd.service")
        .arg("-b")
        .arg("--no-pager")
        .arg("-n")
        .arg("20")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let logs = sanitize_command_output(&String::from_utf8_lossy(&output.stdout));
    if logs.is_empty() {
        None
    } else {
        Some(logs)
    }
}

fn sanitize_command_output(output: &str) -> String {
    let mut cleaned = String::new();
    let mut chars = output.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if matches!(chars.peek(), Some('[')) {
                let _ = chars.next();
                while let Some(next) = chars.next() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                continue;
            }
            continue;
        }

        cleaned.push(ch);
    }

    cleaned.trim().to_string()
}

pub fn disconnect_wifi() -> Result<(), String> {
    let Some(interface_name) = wifi_interface() else {
        return Ok(());
    };

    if iwd_service_state() != IwdServiceState::Active {
        return Ok(());
    }

    let status = Command::new("iwctl")
        .arg("station")
        .arg(&interface_name)
        .arg("disconnect")
        .status()
        .map_err(|error| format!("failed to run iwctl disconnect: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("iwctl disconnect failed with status {status}"))
    }
}

pub fn wifi_interface() -> Option<String> {
    let dir = fs::read_dir("/sys/class/net").ok()?;
    for entry in dir.flatten() {
        let path = entry.path();
        if path.join("wireless").exists() {
            return entry.file_name().to_str().map(str::to_string);
        }
    }
    None
}

fn current_connected_network(interface_name: &str) -> Option<String> {
    let output = run_iwctl(["station", interface_name, "show"]).ok()?;
    for raw_line in output.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Connected network") {
            let value = rest.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn read_connection_info(interface_name: &str) -> WifiConnectionInfo {
    let mut info = WifiConnectionInfo::default();
    let output = run_iwctl(["station", interface_name, "show"]).unwrap_or_default();

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Connected network") {
            let value = rest.trim();
            if !value.is_empty() {
                info.ssid = Some(value.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("IPv4 address") {
            let value = rest.trim();
            if !value.is_empty() {
                info.ipv4_addresses.push(value.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("IPv6 address") {
            let value = rest.trim();
            if !value.is_empty() {
                info.ipv6_addresses.push(value.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("Security") {
            let value = rest.trim();
            if !value.is_empty() {
                info.security = Some(value.to_string());
            }
        }
    }

    if info.ipv4_addresses.is_empty() {
        info.ipv4_addresses = ip_addresses(interface_name, false);
    }
    if info.ipv6_addresses.is_empty() {
        info.ipv6_addresses = ip_addresses(interface_name, true);
    }

    info
}

fn ip_addresses(interface_name: &str, ipv6: bool) -> Vec<String> {
    let mut command = Command::new("ip");
    command.arg(if ipv6 { "-o" } else { "-o" });
    if ipv6 {
        command.args(["-6", "addr", "show", "dev", interface_name, "scope", "global"]);
    } else {
        command.args(["-4", "addr", "show", "dev", interface_name, "scope", "global"]);
    }

    let Ok(output) = command.output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            while let Some(part) = parts.next() {
                if part == "inet" || part == "inet6" {
                    return parts.next().map(|value| value.to_string());
                }
            }
            None
        })
        .collect()
}

fn known_network_names() -> Result<Vec<String>, String> {
    let output = run_iwctl(["known-networks", "list"])?;
    let mut names = Vec::new();
    let mut in_table = false;

    for raw_line in output.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }

        if !in_table {
            if line.contains("Known networks") {
                in_table = true;
            }
            continue;
        }

        if line.trim_start().starts_with('-') || line.contains("Name") {
            continue;
        }

        let cleaned = line.trim();
        if cleaned.is_empty() {
            continue;
        }

        let columns = split_columns(cleaned);
        if let Some(name) = columns.first() {
            let name = name.trim();
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }

    Ok(names)
}

fn active_ethernet_interface() -> Option<String> {
    let dir = fs::read_dir("/sys/class/net").ok()?;
    for entry in dir.flatten() {
        let interface_name = entry.file_name();
        let interface_name = interface_name.to_str()?;
        if interface_name == "lo" {
            continue;
        }

        let path = entry.path();
        if path.join("wireless").exists() {
            continue;
        }

        let carrier = read_trimmed(path.join("carrier"));
        let operstate = read_trimmed(path.join("operstate"));
        if carrier.as_deref() == Some("1")
            && matches!(operstate.as_deref(), Some("up") | Some("unknown"))
        {
            return Some(interface_name.to_string());
        }
    }
    None
}

fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
}

fn iwd_service_state() -> IwdServiceState {
    let output = Command::new("systemctl")
        .arg("show")
        .arg("iwd.service")
        .arg("--property=ActiveState")
        .arg("--value")
        .output();

    let Ok(output) = output else {
        return IwdServiceState::Unknown;
    };

    if !output.status.success() {
        return IwdServiceState::Unknown;
    }

    match String::from_utf8_lossy(&output.stdout).trim() {
        "active" => IwdServiceState::Active,
        "activating" => IwdServiceState::Activating,
        "inactive" => IwdServiceState::Inactive,
        "failed" => IwdServiceState::Failed,
        _ => IwdServiceState::Unknown,
    }
}

fn wait_for_iwd_ready(timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match iwd_service_state() {
            IwdServiceState::Active => return Ok(()),
            IwdServiceState::Failed => {
                return Err(
                    "Wi-Fi service failed to start. Check iwd.service and the read-only rootfs override."
                        .to_string(),
                )
            }
            IwdServiceState::Inactive => {
                return Err("Wi-Fi service is not running.".to_string())
            }
            IwdServiceState::Activating | IwdServiceState::Unknown => {}
        }
        thread::sleep(Duration::from_millis(150));
    }

    match iwd_service_state() {
        IwdServiceState::Active => Ok(()),
        IwdServiceState::Failed => Err(
            "Wi-Fi service failed to start. Check iwd.service and the read-only rootfs override."
                .to_string(),
        ),
        IwdServiceState::Inactive => Err("Wi-Fi service is not running.".to_string()),
        IwdServiceState::Activating | IwdServiceState::Unknown => {
            Err("Wi-Fi service is still starting. Try again in a moment.".to_string())
        }
    }
}

fn run_iwctl<const N: usize>(args: [&str; N]) -> Result<String, String> {
    let mut child = Command::new("iwctl")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("SYSTEMD_COLORS", "0")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to run iwctl: {error}"))?;

    let deadline = Instant::now() + IWCTL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let output = child
                        .wait_with_output()
                        .map_err(|error| format!("failed to collect iwctl timeout output: {error}"))?;
                    let stderr = sanitize_command_output(&String::from_utf8_lossy(&output.stderr));
                    let stdout = sanitize_command_output(&String::from_utf8_lossy(&output.stdout));
                    let details = if !stderr.is_empty() {
                        stderr
                    } else if !stdout.is_empty() {
                        stdout
                    } else {
                        "no additional output".to_string()
                    };
                    return Err(format!(
                        "iwctl timed out after {}s: {details}",
                        IWCTL_TIMEOUT.as_secs()
                    ));
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("failed to wait for iwctl: {error}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to collect iwctl output: {error}"))?;

    let stdout = sanitize_command_output(&String::from_utf8_lossy(&output.stdout));
    let stderr = sanitize_command_output(&String::from_utf8_lossy(&output.stderr));

    if output.status.success() {
        Ok(stdout)
    } else {
        if !stderr.is_empty() {
            Err(format!("iwctl failed: {stderr}"))
        } else if !stdout.is_empty() {
            Err(format!("iwctl failed: {stdout}"))
        } else {
            Err(format!("iwctl failed with status {}", output.status))
        }
    }
}

fn split_columns(input: &str) -> Vec<String> {
    let mut columns = Vec::new();
    let mut current = String::new();
    let mut pending_spaces = 0usize;

    for ch in input.chars() {
        if ch == ' ' || ch == '\t' {
            pending_spaces += 1;
            continue;
        }

        if pending_spaces >= 2 && !current.trim().is_empty() {
            columns.push(current.trim().to_string());
            current.clear();
        } else if pending_spaces == 1 && !current.is_empty() {
            current.push(' ');
        }

        pending_spaces = 0;
        current.push(ch);
    }

    if !current.trim().is_empty() {
        columns.push(current.trim().to_string());
    }

    columns
}