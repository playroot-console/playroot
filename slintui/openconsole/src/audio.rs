use std::fs;
use std::path::Path;

pub const DEFAULT_AUDIO_DEVICE: &str = "hdmi:CARD=vc4hdmi1,DEV=0";
const AUDIO_DEVICE_STATE_PATH: &str = "/data/openconsole/audio-device";
const HDMI_AUDIO_PORTS: [(&str, &str); 2] = [
    ("HDMI-A-2", "hdmi:CARD=vc4hdmi1,DEV=0"),
    ("HDMI-A-1", "hdmi:CARD=vc4hdmi0,DEV=0"),
];

fn drm_port_connected(port_name: &str) -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return false;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !file_name.contains(port_name) {
            continue;
        }

        let status_path = entry.path().join("status");
        let Ok(status) = fs::read_to_string(status_path) else {
            continue;
        };

        if status.trim() == "connected" {
            return true;
        }
    }

    false
}

pub fn detect_preferred_audio_device() -> String {
    for (port_name, audio_device) in HDMI_AUDIO_PORTS {
        if drm_port_connected(port_name) {
            return audio_device.to_string();
        }
    }

    DEFAULT_AUDIO_DEVICE.to_string()
}

pub fn refresh_audio_device_state() -> Result<String, String> {
    let audio_device = detect_preferred_audio_device();
    let path = Path::new(AUDIO_DEVICE_STATE_PATH);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create audio state directory {}: {error}",
                parent.display()
            )
        })?;
    }

    fs::write(path, format!("{audio_device}\n")).map_err(|error| {
        format!(
            "failed to write audio device state {}: {error}",
            path.display()
        )
    })?;

    Ok(audio_device)
}

pub fn write_asoundrc(path: &Path, audio_device: &str) -> Result<(), String> {
    let card_name = audio_device
        .split("CARD=")
        .nth(1)
        .and_then(|value| value.split(',').next())
        .filter(|value| !value.is_empty())
        .unwrap_or("vc4hdmi1");

    let content = format!(
        "pcm.!default {{\n    type plug\n    slave.pcm \"{audio_device}\"\n}}\n\nctl.!default {{\n    type hw\n    card {card_name}\n}}\n"
    );

    fs::write(path, content)
        .map_err(|error| format!("failed to write ALSA config {}: {error}", path.display()))
}