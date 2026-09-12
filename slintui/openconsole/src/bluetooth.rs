use std::collections::BTreeMap;
use std::process::Command;

use serde::Serialize;

use crate::controller::ControllerStore;

const BLUETOOTH_SCAN_DURATION_SECS: u64 = 12;

#[derive(Debug, Clone)]
pub struct BluetoothScanReport {
    pub raw_output: String,
    pub all_devices: Vec<BluetoothDeviceInfo>,
    pub dualsense_devices: Vec<BluetoothDeviceInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BluetoothDeviceInfo {
    pub address: String,
    pub name: String,
    pub paired: bool,
    pub trusted: bool,
    pub connected: bool,
}

pub fn scan_dualsense_devices() -> Result<BluetoothScanReport, String> {
    let script = format!(
        "{{\n        printf 'scan on\\n'\n        sleep {}\n        printf 'devices\\n'\n        printf 'scan off\\n'\n    }} | bluetoothctl 2>&1",
        BLUETOOTH_SCAN_DURATION_SECS
    );
    let raw_output = run_shell_capture(&script)?;
    let mut all_devices = BTreeMap::<String, BluetoothDeviceInfo>::new();
    let mut dualsense_devices = BTreeMap::<String, BluetoothDeviceInfo>::new();

    for line in raw_output.lines() {
        let cleaned_line = sanitize_bluetooth_line(line);
        let Some(rest) = cleaned_line
            .find("Device ")
            .map(|index| &cleaned_line[index + "Device ".len()..])
        else {
            continue;
        };

        let Some((address, name)) = rest.split_once(' ') else {
            continue;
        };

        let details = bluetoothctl_info(address)?;
        let resolved_name = details.name.clone().unwrap_or_else(|| name.to_string());
        let device = BluetoothDeviceInfo {
            address: address.to_string(),
            name: resolved_name.clone(),
            paired: details.paired,
            trusted: details.trusted,
            connected: details.connected,
        };

        let lower_name = resolved_name.to_ascii_lowercase();
        if lower_name.contains("dualsense") || lower_name.contains("wireless controller") {
            dualsense_devices.insert(address.to_string(), device.clone());
        }

        all_devices.insert(address.to_string(), device);
    }

    Ok(BluetoothScanReport {
        raw_output: sanitize_bluetooth_output(&raw_output),
        all_devices: all_devices.into_values().collect(),
        dualsense_devices: dualsense_devices.into_values().collect(),
    })
}

pub fn pair_dualsense_device(address: &str) -> Result<BluetoothDeviceInfo, String> {
    let script = format!(
        r#"bash <<'EOF'
set -euo pipefail
coproc BLUETOOTHCTL {{ bluetoothctl; }}
exec 3>&${{BLUETOOTHCTL[1]}}
exec 4<&${{BLUETOOTHCTL[0]}}

send_command() {{
    printf '%s\n' "$1" >&3
}}

send_command 'agent on'
send_command 'default-agent'
send_command 'pair {address}'

deadline=$((SECONDS + 20))
while IFS= read -r -t 1 line <&4; do
    printf '%s\n' "$line"

    case "$line" in
        *'Authorize service'*|*'Authorize services'*|*'Authorize pairing'*|*'(yes/no)'*)
            send_command 'yes'
            ;;
        *'Pairing successful'*|*'Paired: yes'*|*'Bonded: yes'*)
            break
            ;;
    esac

    if [ "$SECONDS" -ge "$deadline" ]; then
        break
    fi
done

send_command 'trust {address}'
send_command 'connect {address}'
sleep 1
send_command 'connect {address}'
send_command 'info {address}'
send_command 'quit'

connect_deadline=$((SECONDS + 10))
while IFS= read -r -t 1 line <&4; do
    printf '%s\n' "$line"

    case "$line" in
        *'Authorize service'*|*'Authorize services'*|*'Authorize pairing'*|*'(yes/no)'*)
            send_command 'yes'
            ;;
        *'Connected: yes'*|*'ServicesResolved: yes'*|*'Bonded: yes'*|*'Paired: yes'* )
            break
            ;;
        *'connect failed'*|*'Failed to connect:'*)
            if [ "$SECONDS" -ge "$connect_deadline" ]; then
                break
            fi
            ;;
    esac

    if [ "$SECONDS" -ge "$connect_deadline" ]; then
        break
    fi
done
EOF"#
    );
    let output = run_shell_capture(&script)?;
    let details = parse_info_output(&output);

    if !details.paired && !pairing_output_says_success(&output) {
        return Err(format!(
            "pairing verification failed for {address}: {}\nbluetoothctl output:\n{}",
            classify_pairing_failure(&output),
            sanitize_bluetooth_output(&output)
        ));
    }

    Ok(BluetoothDeviceInfo {
        address: address.to_string(),
        name: details
            .name
            .unwrap_or_else(|| "DualSense Wireless Controller".to_string()),
        paired: details.paired,
        trusted: details.trusted,
        connected: details.connected,
    })
}

pub fn connect_dualsense_device(address: &str) -> Result<BluetoothDeviceInfo, String> {
    let script = format!(
        r#"bash <<'EOF'
set -euo pipefail
coproc BLUETOOTHCTL {{ bluetoothctl; }}
exec 3>&${{BLUETOOTHCTL[1]}}
exec 4<&${{BLUETOOTHCTL[0]}}

send_command() {{
    printf '%s\n' "$1" >&3
}}

send_command 'agent on'
send_command 'default-agent'
send_command 'trust {address}'
send_command 'connect {address}'
sleep 1
send_command 'connect {address}'
send_command 'info {address}'
send_command 'quit'

connect_deadline=$((SECONDS + 12))
while IFS= read -r -t 1 line <&4; do
    printf '%s\n' "$line"

    case "$line" in
        *'Authorize service'*|*'Authorize services'*|*'Authorize pairing'*|*'(yes/no)'*)
            send_command 'yes'
            ;;
        *'Connected: yes'*|*'ServicesResolved: yes'*|*'Bonded: yes'*|*'Paired: yes'* )
            break
            ;;
        *'connect failed'*|*'Failed to connect:'*)
            if [ "$SECONDS" -ge "$connect_deadline" ]; then
                break
            fi
            ;;
    esac

    if [ "$SECONDS" -ge "$connect_deadline" ]; then
        break
    fi
done
EOF"#
    );
    let output = run_shell_capture(&script)?;
    let details = parse_info_output(&output);

    if !details.connected {
        return Err(format!(
            "connection verification failed for {address}: bluetoothctl did not report Connected: yes\nbluetoothctl output:\n{}",
            sanitize_bluetooth_output(&output)
        ));
    }

    Ok(BluetoothDeviceInfo {
        address: address.to_string(),
        name: details
            .name
            .unwrap_or_else(|| "DualSense Wireless Controller".to_string()),
        paired: details.paired,
        trusted: details.trusted,
        connected: details.connected,
    })
}

pub fn unpair_dualsense_device(address: &str) -> Result<(), String> {
    let output = run_shell_capture(&format!("bluetoothctl remove {address}"))?;
    let sanitized = sanitize_bluetooth_output(&output);
    let lower = sanitized.to_ascii_lowercase();

    if lower.contains("device has been removed")
        || lower.contains("removed")
        || lower.contains("successful")
    {
        return Ok(());
    }

    Err(format!(
        "bluetoothctl remove did not report success for {address}:\n{sanitized}"
    ))
}

pub fn reconnect_saved_dualsense_devices(store: &ControllerStore) -> Result<(), String> {
    for controller in &store.controllers {
        if controller.template != "ps5-dualsense" {
            continue;
        }

        let Some(address) = controller.bluetooth_address.as_deref() else {
            continue;
        };

        let _ = connect_dualsense_device(address).or_else(|_| pair_dualsense_device(address));
    }

    Ok(())
}

fn bluetoothctl_info(address: &str) -> Result<BluetoothDeviceInfoDetails, String> {
    let output = run_shell_capture(&format!("bluetoothctl info {address}"))?;
    Ok(parse_info_output(&output))
}

#[derive(Debug, Default, Clone)]
struct BluetoothDeviceInfoDetails {
    name: Option<String>,
    paired: bool,
    trusted: bool,
    connected: bool,
}

fn parse_info_output(output: &str) -> BluetoothDeviceInfoDetails {
    let mut details = BluetoothDeviceInfoDetails::default();

    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Name: ") {
            details.name = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("Alias: ") {
            if details.name.is_none() {
                details.name = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Paired: ") {
            details.paired = value.eq_ignore_ascii_case("yes");
        } else if let Some(value) = line.strip_prefix("Trusted: ") {
            details.trusted = value.eq_ignore_ascii_case("yes");
        } else if let Some(value) = line.strip_prefix("Connected: ") {
            details.connected = value.eq_ignore_ascii_case("yes");
        }
    }

    details
}

fn pairing_output_says_success(output: &str) -> bool {
    output.lines().any(|line| {
        let line = line.trim();
        line.eq_ignore_ascii_case("pairing successful")
            || line.eq_ignore_ascii_case("trust succeeded")
            || line.eq_ignore_ascii_case("device paired")
            || line.eq_ignore_ascii_case("paired: yes")
            || line.eq_ignore_ascii_case("bonded: yes")
            || line.eq_ignore_ascii_case("changing pairable on succeeded")
    })
}

fn classify_pairing_failure(output: &str) -> &'static str {
    let lower_output = output.to_ascii_lowercase();

    if lower_output.contains("failed to register agent object") {
        "bluetoothctl could not register an agent in this session"
    } else if lower_output.contains("no agent registered") {
        "bluetoothctl reported no agent was registered"
    } else if lower_output.contains("default agent request successful") {
        "bluetoothctl reported default-agent succeeded but pairing still did not complete"
    } else if lower_output.contains("authorize service") {
        "bluez requested service authorization"
    } else if lower_output.contains("authorize pairing") {
        "bluez requested pairing authorization"
    } else if lower_output.contains("pairing failed") {
        "bluetoothctl reported pairing failed"
    } else if lower_output.contains("connection failed") {
        "bluetoothctl reported connection failed"
    } else if lower_output.contains("not available") {
        "bluetoothctl reported the adapter or device was not available"
    } else if lower_output.contains("org.bluez.error") {
        "bluez returned an org.bluez error"
    } else {
        "pairing did not reach a confirmed paired state"
    }
}

fn sanitize_bluetooth_output(output: &str) -> String {
    output
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' => ch,
            ' '..='~' => ch,
            _ => ' ',
        })
        .collect()
}

fn sanitize_bluetooth_line(line: &str) -> String {
    sanitize_bluetooth_output(line).trim().to_string()
}

fn run_shell_capture(script: &str) -> Result<String, String> {
    let output = Command::new("sh")
        .arg("-lc")
        .arg(script)
        .output()
        .map_err(|error| format!("failed to run bluetoothctl: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("bluetoothctl command failed: {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
