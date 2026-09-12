use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nix::libc;
use nix::poll::{poll, PollFd, PollFlags};

const INPUT_SCAN_INTERVAL: Duration = Duration::from_millis(1000);
const EV_KEY: u16 = 0x01;
const KEY_CAPABILITY_WORD_BITS: u16 = 64;

pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_SPACE: u16 = 57;
pub const KEY_UP: u16 = 103;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const KEY_DOWN: u16 = 108;
pub const KEY_ENTER: u16 = 28;
pub const KEY_ESC: u16 = 1;
pub const KEY_TAB: u16 = 15;
pub const KEY_W: u16 = 17;
pub const KEY_A: u16 = 30;
pub const KEY_S: u16 = 31;
pub const KEY_D: u16 = 32;

#[derive(Debug, Clone)]
pub enum RawKeyboardEvent {
    Connected {
        device_node: String,
        device_name: String,
    },
    Disconnected {
        device_node: String,
        device_name: String,
    },
    Key {
        device_node: String,
        device_name: String,
        key_code: u16,
        value: i32,
        timestamp: u32,
    },
}

pub struct KeyboardBackend {
    devices: HashMap<PathBuf, KeyboardDevice>,
    last_scan_at: Instant,
}

struct KeyboardDevice {
    event_node: String,
    device_name: String,
    file: File,
    pending: Vec<u8>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    time: libc::timeval,
    type_: u16,
    code: u16,
    value: i32,
}

impl KeyboardBackend {
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
            last_scan_at: Instant::now() - INPUT_SCAN_INTERVAL,
        }
    }

    pub fn poll_events(&mut self) -> Vec<RawKeyboardEvent> {
        let mut events = Vec::new();
        self.refresh_devices_with_events(&mut events);

        if self.devices.is_empty() {
            return events;
        }

        let mut poll_fds = Vec::with_capacity(self.devices.len());
        let mut device_paths = Vec::with_capacity(self.devices.len());

        for (device_path, device) in &self.devices {
            poll_fds.push(PollFd::new(
                device.file.as_raw_fd(),
                PollFlags::POLLIN | PollFlags::POLLERR | PollFlags::POLLHUP,
            ));
            device_paths.push(device_path.clone());
        }

        let Ok(_) = poll(&mut poll_fds, 0) else {
            return events;
        };

        let mut disconnected_paths = Vec::new();
        for (index, poll_fd) in poll_fds.iter().enumerate() {
            let Some(revents) = poll_fd.revents() else {
                continue;
            };

            if revents.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) {
                disconnected_paths.push(device_paths[index].clone());
                continue;
            }

            if revents.intersects(PollFlags::POLLIN) {
                let device_path = &device_paths[index];
                let mut remove_device = false;

                if let Some(device) = self.devices.get_mut(device_path) {
                    if let Err(error) = read_keyboard_events(device, &mut events) {
                        if error.kind() != io::ErrorKind::WouldBlock {
                            remove_device = true;
                        }
                    }
                }

                if remove_device {
                    disconnected_paths.push(device_path.clone());
                }
            }
        }

        for device_path in disconnected_paths {
            self.remove_device(device_path, &mut events);
        }

        events
    }

    fn refresh_devices_with_events(&mut self, events: &mut Vec<RawKeyboardEvent>) {
        if self.last_scan_at.elapsed() < INPUT_SCAN_INTERVAL {
            return;
        }

        self.last_scan_at = Instant::now();

        let mut discovered = HashMap::new();
        for (event_node, device_name, device_path) in discover_keyboard_devices() {
            if self.devices.contains_key(&device_path) || discovered.contains_key(&device_path) {
                continue;
            }

            match open_keyboard_device(&event_node, &device_name, &device_path) {
                Ok(device) => {
                    events.push(RawKeyboardEvent::Connected {
                        device_node: event_node.clone(),
                        device_name: device_name.clone(),
                    });
                    discovered.insert(device_path, device);
                }
                Err(error) => {
                    eprintln!(
                        "keyboard backend: failed to open {} ({}): {}",
                        event_node, device_name, error
                    );
                }
            }
        }

        let current_paths: Vec<PathBuf> = self.devices.keys().cloned().collect();
        for path in current_paths {
            if !discovered.contains_key(&path) && !Path::new(&path).exists() {
                self.remove_device(path, events);
            }
        }

        self.devices.extend(discovered);
    }

    fn remove_device(&mut self, device_path: PathBuf, events: &mut Vec<RawKeyboardEvent>) {
        if let Some(device) = self.devices.remove(&device_path) {
            events.push(RawKeyboardEvent::Disconnected {
                device_node: device.event_node,
                device_name: device.device_name,
            });
        }
    }
}

fn read_keyboard_events(
    device: &mut KeyboardDevice,
    events: &mut Vec<RawKeyboardEvent>,
) -> io::Result<()> {
    let mut buffer = [0u8; 256];

    loop {
        match device.file.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => {
                device.pending.extend_from_slice(&buffer[..bytes_read]);
                let event_size = size_of::<InputEvent>();

                while device.pending.len() >= event_size {
                    let raw_event = unsafe {
                        std::ptr::read_unaligned(device.pending.as_ptr() as *const InputEvent)
                    };
                    device.pending.drain(..event_size);

                    if raw_event.type_ != EV_KEY {
                        continue;
                    }

                    let timestamp = ((raw_event.time.tv_sec as u64) * 1000
                        + (raw_event.time.tv_usec as u64 / 1000))
                        .min(u32::MAX as u64) as u32;

                    events.push(RawKeyboardEvent::Key {
                        device_node: device.event_node.clone(),
                        device_name: device.device_name.clone(),
                        key_code: raw_event.code,
                        value: raw_event.value,
                        timestamp,
                    });
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

fn discover_keyboard_devices() -> Vec<(String, String, PathBuf)> {
    let mut devices = Vec::new();
    let Ok(contents) = fs::read_to_string("/proc/bus/input/devices") else {
        return devices;
    };

    for block in contents.split("\n\n") {
        let Some(handlers) = parse_proc_bus_value(block, "H") else {
            continue;
        };

        if !handlers.split_whitespace().any(|handler| handler == "kbd") {
            continue;
        }

        let Some(event_node) = handlers
            .split_whitespace()
            .find(|handler| handler.starts_with("event"))
            .map(str::to_string)
        else {
            continue;
        };

        let device_name = parse_proc_bus_name(block).unwrap_or_else(|| event_node.clone());
        if !looks_like_physical_keyboard(block, &device_name) {
            continue;
        }

        let device_path = PathBuf::from("/dev/input").join(&event_node);
        devices.push((event_node, device_name, device_path));
    }

    devices
}

fn looks_like_physical_keyboard(block: &str, device_name: &str) -> bool {
    let lower_name = device_name.to_ascii_lowercase();
    if lower_name.contains("keyboard") {
        return true;
    }

    let looks_like_controller = [
        "gamepad",
        "controller",
        "joystick",
        "dualsense",
        "xbox",
        "snes",
        "nes",
        "8bitdo",
    ]
    .iter()
    .any(|term| lower_name.contains(term));

    let Some(key_bitmap) = parse_proc_bus_capability(block, "KEY") else {
        return !looks_like_controller;
    };

    let alpha_count = [KEY_W, KEY_A, KEY_S, KEY_D]
        .into_iter()
        .filter(|key_code| key_bitmap_has_code(&key_bitmap, *key_code))
        .count();
    let navigation_count = [KEY_UP, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_ENTER, KEY_SPACE]
        .into_iter()
        .filter(|key_code| key_bitmap_has_code(&key_bitmap, *key_code))
        .count();

    if looks_like_controller {
        return false;
    }

    alpha_count >= 2 && navigation_count >= 2
}

fn parse_proc_bus_capability(block: &str, capability: &str) -> Option<String> {
    let prefix = format!("B: {capability}=");
    block.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(str::trim)
            .map(str::to_string)
    })
}

fn key_bitmap_has_code(bitmap: &str, key_code: u16) -> bool {
    let words = bitmap.split_whitespace().collect::<Vec<_>>();
    let word_index = usize::from(key_code / KEY_CAPABILITY_WORD_BITS);
    if word_index >= words.len() {
        return false;
    }

    let Some(reversed_index) = words.len().checked_sub(word_index + 1) else {
        return false;
    };
    let Ok(word) = u64::from_str_radix(words[reversed_index], 16) else {
        return false;
    };

    let bit_index = u32::from(key_code % KEY_CAPABILITY_WORD_BITS);
    word & (1u64 << bit_index) != 0
}

pub fn keyboard_device_candidates() -> Vec<(String, String)> {
    discover_keyboard_devices()
        .into_iter()
        .map(|(event_node, device_name, _)| (event_node, device_name))
        .collect()
}

fn open_keyboard_device(
    event_node: &str,
    device_name: &str,
    device_path: &Path,
) -> io::Result<KeyboardDevice> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(device_path)?;

    Ok(KeyboardDevice {
        event_node: event_node.to_string(),
        device_name: device_name.to_string(),
        file,
        pending: Vec::new(),
    })
}

fn parse_proc_bus_value(block: &str, prefix: &str) -> Option<String> {
    let prefix = format!("{prefix}: ");
    block.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(str::trim)
            .map(str::to_string)
    })
}

fn parse_proc_bus_name(block: &str) -> Option<String> {
    parse_proc_bus_value(block, "N").map(|value| {
        value
            .strip_prefix("Name=")
            .unwrap_or(&value)
            .trim_matches('"')
            .to_string()
    })
}

pub fn key_code_name(key_code: u16) -> &'static str {
    match key_code {
        KEY_ESC => "KEY_ESC",
        KEY_TAB => "KEY_TAB",
        KEY_ENTER => "KEY_ENTER",
        KEY_SPACE => "KEY_SPACE",
        KEY_LEFTCTRL => "KEY_LEFTCTRL",
        KEY_RIGHTCTRL => "KEY_RIGHTCTRL",
        KEY_UP => "KEY_UP",
        KEY_LEFT => "KEY_LEFT",
        KEY_RIGHT => "KEY_RIGHT",
        KEY_DOWN => "KEY_DOWN",
        KEY_W => "KEY_W",
        KEY_A => "KEY_A",
        KEY_S => "KEY_S",
        KEY_D => "KEY_D",
        _ => "KEY_UNKNOWN",
    }
}
