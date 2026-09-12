use std::fs::File;
use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Duration;
#[cfg(not(feature = "sdl-input"))]
use std::time::Instant;

use evdev::{AbsoluteAxisType, Device, InputEvent, InputEventKind, Key};
use nix::fcntl::{fcntl, FcntlArg, OFlag};

#[cfg(not(feature = "sdl-input"))]
use crate::logging::log_launch;

#[cfg(not(feature = "sdl-input"))]
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const JS_EVENT_BUTTON: u8 = 0x01;
const JS_EVENT_AXIS: u8 = 0x02;
const JS_EVENT_INIT: u8 = 0x80;

pub struct JoystickDevice {
    file: File,
}

pub struct HidRawDevice {
    file: File,
    last_report: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct ControllerInputDeviceInfo {
    pub event_node: String,
    pub name: String,
    pub unique_id: String,
    pub physical_path: String,
}

#[cfg(feature = "sdl-input")]
fn from_sdl_device_info(device: crate::input::ControllerDeviceInfo) -> ControllerInputDeviceInfo {
    ControllerInputDeviceInfo {
        event_node: device.event_node,
        name: device.name,
        unique_id: device.unique_id,
        physical_path: device.physical_path,
    }
}

pub fn capture_controller_input(timeout: Duration) -> Result<String, String> {
    capture_controller_input_for_device(timeout, None)
}

pub fn capture_controller_input_for_device(timeout: Duration, selected_device_id: Option<&str>) -> Result<String, String> {
    #[cfg(feature = "sdl-input")]
    {
        return crate::input::capture_controller_input_for_device(timeout, selected_device_id);
    }

    #[cfg(not(feature = "sdl-input"))]
    let mut devices = open_controller_input_devices_for_device(true, selected_device_id);
    #[cfg(not(feature = "sdl-input"))]
    let mut joystick_devices = open_joystick_input_devices_for_device(selected_device_id);
    #[cfg(not(feature = "sdl-input"))]
    let mut hidraw_devices = open_hidraw_gamepad_devices_for_device(selected_device_id);

    #[cfg(not(feature = "sdl-input"))]
    if !devices.is_empty() || !joystick_devices.is_empty() {
        hidraw_devices.clear();
    }

    #[cfg(not(feature = "sdl-input"))]
    log_launch(&format!(
        "controller capture: opened {} evdev, {} joystick and {} hidraw devices",
        devices.len(),
        joystick_devices.len(),
        hidraw_devices.len()
    ));

    #[cfg(not(feature = "sdl-input"))]
    if devices.is_empty() && joystick_devices.is_empty() && hidraw_devices.is_empty() {
        return Err("no controller input devices found under /dev/input".to_string());
    }

    #[cfg(not(feature = "sdl-input"))]
    let deadline = Instant::now() + timeout;
    #[cfg(not(feature = "sdl-input"))]
    let mut debug_token_count = 0usize;
    #[cfg(not(feature = "sdl-input"))]
    while Instant::now() < deadline {
        #[cfg(not(feature = "sdl-input"))]
        for device in &mut devices {
            match device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        for (token, value) in normalize_event_tokens(&event) {
                            if debug_token_count < 32 {
                                log_launch(&format!("controller capture evdev token: {token}={value}"));
                                debug_token_count += 1;
                            }
                            if value == 1 {
                                return Ok(token);
                            }
                        }
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(_) => {}
            }
        }

        #[cfg(not(feature = "sdl-input"))]
        for joystick in &mut joystick_devices {
            for (token, value) in read_joystick_tokens(joystick) {
                if debug_token_count < 32 {
                    log_launch(&format!("controller capture js token: {token}={value}"));
                    debug_token_count += 1;
                }
                if value == 1 {
                    return Ok(token);
                }
            }
        }

        #[cfg(not(feature = "sdl-input"))]
        for hidraw in &mut hidraw_devices {
            for (token, value) in read_hidraw_tokens(hidraw) {
                if debug_token_count < 32 {
                    log_launch(&format!("controller capture hidraw token: {token}={value}"));
                    debug_token_count += 1;
                }
                if value == 1 {
                    return Ok(token);
                }
            }
        }

        #[cfg(not(feature = "sdl-input"))]
        std::thread::sleep(POLL_INTERVAL);
    }

    #[cfg(not(feature = "sdl-input"))]
    Err("timeout waiting for controller input".to_string())
}

pub fn list_controller_input_devices() -> Vec<ControllerInputDeviceInfo> {
    #[cfg(feature = "sdl-input")]
    {
        return crate::input::list_controller_input_devices()
            .into_iter()
            .map(from_sdl_device_info)
            .collect();
    }

    #[cfg(not(feature = "sdl-input"))]
    list_controller_input_devices_with_usb_fallback()
}

pub fn list_all_controller_input_devices() -> Vec<ControllerInputDeviceInfo> {
    #[cfg(feature = "sdl-input")]
    {
        return crate::input::list_all_controller_input_devices()
            .into_iter()
            .map(from_sdl_device_info)
            .collect();
    }

    #[cfg(not(feature = "sdl-input"))]
    collect_controller_input_devices(false)
}

pub fn open_controller_input_devices(grab: bool) -> Vec<Device> {
    open_controller_input_devices_for_device(grab, None)
}

pub fn open_controller_input_devices_for_device(grab: bool, selected_device_id: Option<&str>) -> Vec<Device> {
    let mut devices = Vec::new();

    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return devices;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
            continue;
        };

        if !file_name.starts_with("event") {
            continue;
        }

        match Device::open(&path) {
            Ok(mut device) => {
                if !device_looks_like_controller(&device) {
                    continue;
                }

                if let Some(selected_device_id) = selected_device_id {
                    let aliases = controller_device_aliases(&path, &device);
                    if !device_id_matches_aliases(selected_device_id, &aliases) {
                        continue;
                    }
                }

                if fcntl(device.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).is_err() {
                    continue;
                }

                if grab {
                    let _ = device.grab();
                }

                devices.push(device);
            }
            Err(_) => continue,
        }
    }

    devices
}

#[cfg(not(feature = "sdl-input"))]
fn list_controller_input_devices_with_usb_fallback() -> Vec<ControllerInputDeviceInfo> {
    let usb_devices = collect_controller_input_devices(true);
    if !usb_devices.is_empty() {
        return usb_devices;
    }

    collect_controller_input_devices(false)
}

#[cfg(not(feature = "sdl-input"))]
fn collect_controller_input_devices(usb_only: bool) -> Vec<ControllerInputDeviceInfo> {
    let mut devices = Vec::new();

    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return devices;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
            continue;
        };

        if !file_name.starts_with("event") {
            continue;
        }

        let Ok(device) = Device::open(&path) else {
            continue;
        };

        if !device_looks_like_controller(&device) {
            continue;
        }

        if usb_only && !device_looks_like_usb(&path, &device) {
            continue;
        }

        devices.push(controller_device_info(&path, &device));
    }

    devices.sort_by(|left, right| left.event_node.cmp(&right.event_node));
    devices
}

#[cfg(not(feature = "sdl-input"))]
fn controller_device_info(path: &Path, device: &Device) -> ControllerInputDeviceInfo {
    let event_node = path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or("event?")
        .to_string();
    let name = device.name().unwrap_or("Unknown controller").to_string();
    let physical_path = device.physical_path().unwrap_or("").to_string();
    let unique_id = controller_device_unique_id(path, device);

    ControllerInputDeviceInfo {
        event_node,
        name,
        unique_id,
        physical_path,
    }
}

fn controller_device_unique_id(path: &Path, device: &Device) -> String {
    if let Some(physical_path) = device.physical_path() {
        if !physical_path.is_empty() {
            return format!("phys:{physical_path}");
        }
    }

    if let Some(unique_name) = device.unique_name() {
        if !unique_name.is_empty() {
            return format!("uniq:{unique_name}");
        }
    }

    controller_device_identity_for_path(path).unwrap_or_else(|| {
        format!("event:{}", path.file_name().and_then(|file_name| file_name.to_str()).unwrap_or("unknown"))
    })
}

fn controller_device_aliases(path: &Path, device: &Device) -> Vec<String> {
    let mut aliases = vec![controller_device_unique_id(path, device)];

    if let Some(name) = device.name() {
        aliases.push(format!("kind:joystick|name:{name}"));
        aliases.push(format!("joystick-name:{name}"));
        aliases.push(format!("controller-name:{name}"));
    }

    aliases.sort();
    aliases.dedup();
    aliases
}

fn device_id_matches_aliases(selected_device_id: &str, device_aliases: &[String]) -> bool {
    let selected_aliases = device_id_aliases(selected_device_id);
    selected_aliases
        .iter()
        .any(|selected| device_aliases.iter().any(|device| device == selected))
}

fn device_id_aliases(device_id: &str) -> Vec<String> {
    let mut aliases = vec![device_id.to_string()];

    let kind = segment_value(device_id, "kind:");
    let name = segment_value(device_id, "name:");
    let vendor = segment_value(device_id, "vendor:");
    let product = segment_value(device_id, "product:");
    let guid = segment_value(device_id, "guid:");
    let legacy_joystick = segment_value(device_id, "joystick-instance:").is_some();
    let legacy_controller = segment_value(device_id, "instance:").is_some();

    if let (Some(name), Some(vendor), Some(product)) = (&name, &vendor, &product) {
        aliases.push(format!(
            "kind:controller|name:{name}|vendor:{vendor}|product:{product}"
        ));
        aliases.push(format!("controller-name:{name}|vendor:{vendor}|product:{product}"));
    } else if (kind.as_deref() == Some("controller") || legacy_controller) && name.is_some() {
        let name = name.as_deref().unwrap_or_default();
        aliases.push(format!("controller-name:{name}"));
    }

    if let Some(name) = &name {
        if guid.is_some() || kind.as_deref() == Some("joystick") || legacy_joystick {
            aliases.push(format!("kind:joystick|name:{name}"));
            aliases.push(format!("joystick-name:{name}"));
        }
    }

    if let (Some(guid), Some(name)) = (&guid, &name) {
        aliases.push(format!("kind:joystick|guid:{guid}|name:{name}"));
    }

    aliases.sort();
    aliases.dedup();
    aliases
}

fn segment_value(device_id: &str, prefix: &str) -> Option<String> {
    device_id
        .split('|')
        .find_map(|segment| segment.strip_prefix(prefix).map(str::to_string))
}

#[cfg(not(feature = "sdl-input"))]
fn device_looks_like_usb(path: &Path, device: &Device) -> bool {
    let mut haystack = String::new();
    haystack.push_str(&path.display().to_string());
    if let Some(name) = device.name() {
        haystack.push(' ');
        haystack.push_str(name);
    }
    if let Some(physical_path) = device.physical_path() {
        haystack.push(' ');
        haystack.push_str(physical_path);
    }
    if let Some(unique_name) = device.unique_name() {
        haystack.push(' ');
        haystack.push_str(unique_name);
    }

    haystack.to_ascii_lowercase().contains("usb")
}

pub fn open_joystick_input_devices_for_device(selected_device_id: Option<&str>) -> Vec<JoystickDevice> {
    let mut devices = Vec::new();

    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return devices;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
            continue;
        };

        if !file_name.starts_with("js") {
            continue;
        }

        if let Some(selected_device_id) = selected_device_id {
            if controller_joystick_identity(file_name) != Some(selected_device_id.to_string()) {
                continue;
            }
        }

        let Ok(file) = File::open(&path) else {
            continue;
        };

        if fcntl(file.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).is_err() {
            continue;
        }

        devices.push(JoystickDevice { file });
    }

    devices
}

pub fn open_hidraw_gamepad_devices_for_device(selected_device_id: Option<&str>) -> Vec<HidRawDevice> {
    let mut devices = Vec::new();

    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        return devices;
    };

    for entry in entries.flatten() {
        let hidraw_name = entry.file_name();
        let hidraw_name = hidraw_name.to_string_lossy().to_string();
        let uevent_path = entry.path().join("device").join("uevent");
        let Ok(uevent) = fs::read_to_string(&uevent_path) else {
            continue;
        };

        let lower = uevent.to_ascii_lowercase();
        if !lower.contains("gamepad") {
            continue;
        }

        if let Some(selected_device_id) = selected_device_id {
            if controller_hidraw_identity(&hidraw_name) != Some(selected_device_id.to_string()) {
                continue;
            }
        }

        let dev_path = format!("/dev/{hidraw_name}");
        let Ok(file) = File::open(&dev_path) else {
            continue;
        };

        if fcntl(file.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).is_err() {
            continue;
        }

        devices.push(HidRawDevice {
            file,
            last_report: None,
        });
    }

    devices
}

fn controller_device_identity_for_path(path: &Path) -> Option<String> {
    let node_name = path.file_name()?.to_str()?;
    controller_input_node_identity(node_name)
}

pub fn controller_joystick_identity(node_name: &str) -> Option<String> {
    controller_input_node_identity(node_name)
}

pub fn controller_hidraw_identity(node_name: &str) -> Option<String> {
    let sysfs_path = format!("/sys/class/hidraw/{node_name}/device");
    let resolved = fs::canonicalize(&sysfs_path).ok()?;
    resolved.parent().map(|parent| parent.to_string_lossy().to_string())
}

pub fn controller_device_matches_id(path: &Path, device: &Device, selected_device_id: &str) -> bool {
    controller_device_unique_id(path, device) == selected_device_id
}

fn controller_input_node_identity(node_name: &str) -> Option<String> {
    let sysfs_path = format!("/sys/class/input/{node_name}/device");
    let resolved = fs::canonicalize(&sysfs_path).ok()?;
    resolved.parent().map(|parent| parent.to_string_lossy().to_string())
}

pub fn open_joystick_input_devices() -> Vec<JoystickDevice> {
    let mut devices = Vec::new();

    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return devices;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
            continue;
        };

        if !file_name.starts_with("js") {
            continue;
        }

        let Ok(file) = File::open(&path) else {
            continue;
        };

        if fcntl(file.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).is_err() {
            continue;
        }

        devices.push(JoystickDevice { file });
    }

    devices
}

pub fn read_joystick_tokens(device: &mut JoystickDevice) -> Vec<(String, i32)> {
    let mut out = Vec::new();

    loop {
        let mut buf = [0u8; 8];
        match device.file.read_exact(&mut buf) {
            Ok(()) => {
                if buf[6] & JS_EVENT_INIT != 0 {
                    continue;
                }

                let value = i16::from_le_bytes([buf[4], buf[5]]) as i32;
                let event_type = buf[6] & !JS_EVENT_INIT;
                let number = buf[7];

                if event_type == JS_EVENT_BUTTON {
                    let token = format!("JS_BTN_{number}");
                    out.push((token, if value != 0 { 1 } else { 0 }));
                    continue;
                }

                if event_type == JS_EVENT_AXIS {
                    let neg = format!("JS_AXIS_{}_NEG", number);
                    let pos = format!("JS_AXIS_{}_POS", number);
                    if value <= -10_000 {
                        out.push((neg, 1));
                        out.push((pos, 0));
                    } else if value >= 10_000 {
                        out.push((neg, 0));
                        out.push((pos, 1));
                    } else {
                        out.push((neg, 0));
                        out.push((pos, 0));
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }

    out
}

pub fn open_hidraw_gamepad_devices() -> Vec<HidRawDevice> {
    let mut devices = Vec::new();

    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        return devices;
    };

    for entry in entries.flatten() {
        let hidraw_name = entry.file_name();
        let hidraw_name = hidraw_name.to_string_lossy().to_string();
        let uevent_path = entry.path().join("device").join("uevent");
        let Ok(uevent) = fs::read_to_string(&uevent_path) else {
            continue;
        };

        let lower = uevent.to_ascii_lowercase();
        if !lower.contains("gamepad") {
            continue;
        }

        let dev_path = format!("/dev/{hidraw_name}");
        let Ok(file) = File::open(&dev_path) else {
            continue;
        };

        if fcntl(file.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).is_err() {
            continue;
        }

        devices.push(HidRawDevice {
            file,
            last_report: None,
        });
    }

    devices
}

pub fn read_hidraw_tokens(device: &mut HidRawDevice) -> Vec<(String, i32)> {
    let mut out = Vec::new();

    loop {
        let mut buf = [0u8; 64];
        match device.file.read(&mut buf) {
            Ok(0) => break,
            Ok(size) => {
                let current = buf[..size].to_vec();
                if let Some(previous) = &device.last_report {
                    let compare_len = previous.len().min(current.len());
                    for idx in 0..compare_len {
                        let old = previous[idx];
                        let new = current[idx];
                        if old == new {
                            continue;
                        }

                        for bit in 0..8 {
                            let old_bit = (old >> bit) & 1;
                            let new_bit = (new >> bit) & 1;
                            if old_bit != new_bit {
                                let token = format!("HIDRAW_B{}_BIT{}", idx, bit);
                                out.push((token, i32::from(new_bit)));
                            }
                        }
                    }
                }

                device.last_report = Some(current);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }

    out
}

pub fn device_looks_like_controller(device: &Device) -> bool {
    let has_gamepad_button = device
        .supported_keys()
        .map(|keys| {
            [
                Key::BTN_SOUTH,
                Key::BTN_EAST,
                Key::BTN_NORTH,
                Key::BTN_WEST,
                Key::BTN_START,
                Key::BTN_SELECT,
                Key::BTN_DPAD_UP,
                Key::BTN_DPAD_DOWN,
                Key::BTN_DPAD_LEFT,
                Key::BTN_DPAD_RIGHT,
                Key::BTN_TRIGGER,
                Key::BTN_THUMB,
                Key::BTN_THUMB2,
                Key::BTN_TOP,
                Key::BTN_TOP2,
                Key::BTN_PINKIE,
                Key::BTN_BASE,
                Key::BTN_BASE2,
                Key::BTN_BASE3,
                Key::BTN_BASE4,
            ]
            .iter()
            .any(|key| keys.contains(*key))
        })
        .unwrap_or(false);

    let has_direction_axes = device
        .supported_absolute_axes()
        .map(|axes| {
            [
                AbsoluteAxisType::ABS_HAT0X,
                AbsoluteAxisType::ABS_HAT0Y,
                AbsoluteAxisType::ABS_X,
                AbsoluteAxisType::ABS_Y,
                AbsoluteAxisType::ABS_RX,
                AbsoluteAxisType::ABS_RY,
            ]
            .iter()
            .any(|axis| axes.contains(*axis))
        })
        .unwrap_or(false);

    has_gamepad_button || has_direction_axes
}

pub fn normalize_event_tokens(event: &InputEvent) -> Vec<(String, i32)> {
    match event.kind() {
        InputEventKind::Key(key) => {
            let value = event.value();
            if value == 0 || value == 1 {
                vec![(normalize_key(key), value)]
            } else {
                Vec::new()
            }
        }
        InputEventKind::AbsAxis(axis) => normalize_abs_axis(axis, event.value()),
        _ => Vec::new(),
    }
}

fn normalize_abs_axis(axis: AbsoluteAxisType, value: i32) -> Vec<(String, i32)> {
    match axis {
        AbsoluteAxisType::ABS_HAT0X => match value {
            -1 => vec![("LeftArrow".to_string(), 1), ("RightArrow".to_string(), 0)],
            1 => vec![("LeftArrow".to_string(), 0), ("RightArrow".to_string(), 1)],
            _ => vec![("LeftArrow".to_string(), 0), ("RightArrow".to_string(), 0)],
        },
        AbsoluteAxisType::ABS_HAT0Y => match value {
            -1 => vec![("UpArrow".to_string(), 1), ("DownArrow".to_string(), 0)],
            1 => vec![("UpArrow".to_string(), 0), ("DownArrow".to_string(), 1)],
            _ => vec![("UpArrow".to_string(), 0), ("DownArrow".to_string(), 0)],
        },
        AbsoluteAxisType::ABS_X | AbsoluteAxisType::ABS_RX => {
            if value >= 0 && value <= 255 {
                if value <= 64 {
                    vec![("LeftArrow".to_string(), 1), ("RightArrow".to_string(), 0)]
                } else if value >= 192 {
                    vec![("LeftArrow".to_string(), 0), ("RightArrow".to_string(), 1)]
                } else {
                    vec![("LeftArrow".to_string(), 0), ("RightArrow".to_string(), 0)]
                }
            } else if value <= -16384 {
                vec![("LeftArrow".to_string(), 1), ("RightArrow".to_string(), 0)]
            } else if value >= 16384 {
                vec![("LeftArrow".to_string(), 0), ("RightArrow".to_string(), 1)]
            } else {
                vec![("LeftArrow".to_string(), 0), ("RightArrow".to_string(), 0)]
            }
        }
        AbsoluteAxisType::ABS_Y | AbsoluteAxisType::ABS_RY => {
            if value >= 0 && value <= 255 {
                if value <= 64 {
                    vec![("UpArrow".to_string(), 1), ("DownArrow".to_string(), 0)]
                } else if value >= 192 {
                    vec![("UpArrow".to_string(), 0), ("DownArrow".to_string(), 1)]
                } else {
                    vec![("UpArrow".to_string(), 0), ("DownArrow".to_string(), 0)]
                }
            } else if value <= -16384 {
                vec![("UpArrow".to_string(), 1), ("DownArrow".to_string(), 0)]
            } else if value >= 16384 {
                vec![("UpArrow".to_string(), 0), ("DownArrow".to_string(), 1)]
            } else {
                vec![("UpArrow".to_string(), 0), ("DownArrow".to_string(), 0)]
            }
        }
        _ => Vec::new(),
    }
}

pub fn normalize_key(key: Key) -> String {
    match key {
        Key::KEY_UP | Key::BTN_DPAD_UP => "UpArrow".to_string(),
        Key::KEY_DOWN | Key::BTN_DPAD_DOWN => "DownArrow".to_string(),
        Key::KEY_LEFT | Key::BTN_DPAD_LEFT => "LeftArrow".to_string(),
        Key::KEY_RIGHT | Key::BTN_DPAD_RIGHT => "RightArrow".to_string(),
        Key::KEY_W => "W".to_string(),
        Key::KEY_A => "A".to_string(),
        Key::KEY_S => "S".to_string(),
        Key::KEY_D => "D".to_string(),
        Key::KEY_SPACE
        | Key::BTN_SOUTH
        | Key::BTN_WEST
        | Key::BTN_TRIGGER
        | Key::BTN_THUMB => "Space".to_string(),
        Key::KEY_ENTER
        | Key::KEY_KPENTER
        | Key::BTN_NORTH
        | Key::BTN_TRIGGER_HAPPY1
        | Key::BTN_TRIGGER_HAPPY2
        | Key::BTN_THUMB2 => "Return".to_string(),
        Key::KEY_ESC | Key::BTN_START | Key::BTN_MODE | Key::BTN_EAST => "Escape".to_string(),
        Key::BTN_SELECT | Key::BTN_BASE => "Return".to_string(),
        other => format!("{other:?}"),
    }
}
