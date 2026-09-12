use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::Event;
use sdl2::joystick::Joystick;
use sdl2::keyboard::Keycode;

use crate::controller::{controller_store_revision, load_store};
use crate::keyboard_input::{
    key_code_name, keyboard_device_candidates, KeyboardBackend, RawKeyboardEvent,
};
use crate::logging::log_launch;

const DEFAULT_INITIAL_REPEAT_DELAY: Duration = Duration::from_millis(300);
const DEFAULT_REPEAT_INTERVAL: Duration = Duration::from_millis(100);
const POLL_INTERVAL: Duration = Duration::from_millis(8);
const DEAD_ZONE: i16 = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsoleAction {
    Up,
    Down,
    Left,
    Right,
    Accept,
    Cancel,
    Start,
    Select,
}

impl ConsoleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Accept => "Accept",
            Self::Cancel => "Cancel",
            Self::Start => "Start",
            Self::Select => "Select",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleInputPhase {
    Pressed,
    Repeat,
    Released,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub enum ConsoleInputSource {
    Keyboard {
        keycode: String,
        device_name: String,
    },
    Controller {
        instance_id: u32,
        device_id: String,
        controller_name: String,
    },
}

#[derive(Debug, Clone)]
pub struct ConsoleInputEvent {
    pub phase: ConsoleInputPhase,
    pub source: ConsoleInputSource,
    pub action: Option<ConsoleAction>,
    pub token: String,
    pub pressed: bool,
    pub timestamp: u32,
    pub controller_name: String,
}

#[derive(Debug, Clone)]
pub struct ControllerDeviceInfo {
    pub event_node: String,
    pub name: String,
    pub unique_id: String,
    pub physical_path: String,
}

static RUNTIME_CONTROLLER_DEVICES: OnceLock<Mutex<Vec<ControllerDeviceInfo>>> = OnceLock::new();

pub struct ConsoleInputHandle {
    stop: std::sync::Arc<AtomicBool>,
    join_handle: thread::JoinHandle<()>,
}

impl ConsoleInputHandle {
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.join_handle.join();
    }
}

#[derive(Default)]
struct ActionState {
    active_sources: HashSet<SourceId>,
    next_repeat_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SourceId {
    Keyboard {
        device_node: String,
        key_code: u16,
    },
    ControllerButton(u32, Button),
    ControllerAxis {
        instance_id: u32,
        axis: Axis,
        direction: i8,
    },
    JoystickButton {
        instance_id: u32,
        button_idx: u8,
    },
    JoystickAxis {
        instance_id: u32,
        axis_idx: u8,
        direction: i8,
    },
}

fn describe_source(source: &SourceId) -> String {
    match source {
        SourceId::Keyboard {
            device_node,
            key_code,
        } => format!("Keyboard(node={device_node},code={key_code})"),
        SourceId::ControllerButton(instance_id, button) => {
            format!("ControllerButton(instance={instance_id},button={button:?})")
        }
        SourceId::ControllerAxis {
            instance_id,
            axis,
            direction,
        } => format!(
            "ControllerAxis(instance={instance_id},axis={axis:?},direction={direction})"
        ),
        SourceId::JoystickButton {
            instance_id,
            button_idx,
        } => format!("JoystickButton(instance={instance_id},button={button_idx})"),
        SourceId::JoystickAxis {
            instance_id,
            axis_idx,
            direction,
        } => format!(
            "JoystickAxis(instance={instance_id},axis={axis_idx},direction={direction})"
        ),
    }
}

fn describe_sources(sources: &HashSet<SourceId>) -> String {
    let mut descriptions = sources.iter().map(describe_source).collect::<Vec<_>>();
    descriptions.sort();
    descriptions.join(", ")
}

struct OpenJoystick {
    joystick: Joystick,
    device_id: String,
    pending_neutral_axes: HashSet<u8>,
    axis_input_ready: bool,
}

type ControllerProfileBindings = HashMap<String, HashMap<String, ConsoleAction>>;

fn runtime_controller_devices() -> &'static Mutex<Vec<ControllerDeviceInfo>> {
    RUNTIME_CONTROLLER_DEVICES.get_or_init(|| Mutex::new(Vec::new()))
}

fn snapshot_runtime_controller_devices() -> Vec<ControllerDeviceInfo> {
    runtime_controller_devices()
        .lock()
        .map(|devices| devices.clone())
        .unwrap_or_default()
}

fn replace_runtime_controller_devices(devices: Vec<ControllerDeviceInfo>) {
    if let Ok(mut guard) = runtime_controller_devices().lock() {
        *guard = devices;
    }
}

fn controller_device_info_from_controller(
    index: u32,
    controller: &GameController,
) -> ControllerDeviceInfo {
    ControllerDeviceInfo {
        event_node: format!("sdl:{index}"),
        name: controller.name(),
        unique_id: controller_unique_id(controller),
        physical_path: format!("SDL controller index {index}"),
    }
}

fn controller_device_info_from_joystick(index: u32, joystick: &Joystick) -> ControllerDeviceInfo {
    ControllerDeviceInfo {
        event_node: format!("sdl-joystick:{index}"),
        name: joystick.name(),
        unique_id: joystick_unique_id(joystick),
        physical_path: format!(
            "SDL joystick index {index} axes={} buttons={} hats={}",
            joystick.num_axes(),
            joystick.num_buttons(),
            joystick.num_hats()
        ),
    }
}

fn fallback_controller_setup_devices(reason: &str) -> Vec<ControllerDeviceInfo> {
    let cached_devices = snapshot_runtime_controller_devices();
    if cached_devices.is_empty() {
        log_launch(&format!(
            "SDL device enumeration: {reason}; no cached runtime devices available"
        ));
    } else {
        log_launch(&format!(
            "SDL device enumeration: {reason}; using {} cached runtime device(s) for controller setup",
            cached_devices.len()
        ));
    }
    cached_devices
}

fn collect_runtime_controller_devices(
    controllers: &HashMap<u32, GameController>,
    joysticks: &HashMap<u32, OpenJoystick>,
) -> Vec<ControllerDeviceInfo> {
    let mut devices = controllers
        .values()
        .map(|controller| ControllerDeviceInfo {
            event_node: format!("sdl-instance:{}", controller.instance_id()),
            name: controller.name(),
            unique_id: controller_unique_id(controller),
            physical_path: format!("SDL controller instance {}", controller.instance_id()),
        })
        .collect::<Vec<_>>();

    devices.extend(joysticks.values().map(|joystick| ControllerDeviceInfo {
        event_node: format!("sdl-joystick-instance:{}", joystick.joystick.instance_id()),
        name: joystick.joystick.name(),
        unique_id: joystick.device_id.clone(),
        physical_path: format!(
            "SDL joystick instance {} axes={} buttons={} hats={}",
            joystick.joystick.instance_id(),
            joystick.joystick.num_axes(),
            joystick.joystick.num_buttons(),
            joystick.joystick.num_hats()
        ),
    }));

    devices.sort_by(|left, right| left.unique_id.cmp(&right.unique_id));
    devices
}

fn action_from_mapping_label(label: &str) -> Option<ConsoleAction> {
    if label.starts_with("Move Up") {
        Some(ConsoleAction::Up)
    } else if label.starts_with("Move Down") {
        Some(ConsoleAction::Down)
    } else if label.starts_with("Move Left") {
        Some(ConsoleAction::Left)
    } else if label.starts_with("Move Right") {
        Some(ConsoleAction::Right)
    } else if label.starts_with("Button A")
        || label.starts_with("Button X")
        || label.starts_with("Button Cross")
    {
        Some(ConsoleAction::Accept)
    } else if label.starts_with("Button B")
        || label.starts_with("Button Y")
        || label.starts_with("Button Circle")
    {
        Some(ConsoleAction::Cancel)
    } else if label.starts_with("Button Create")
        || label.starts_with("Button Touchpad")
        || label.starts_with("Top Left")
        || label.starts_with("Select")
    {
        Some(ConsoleAction::Select)
    } else if label.starts_with("Button Options")
        || label.starts_with("Top Right")
        || label.starts_with("Start")
    {
        Some(ConsoleAction::Start)
    } else {
        None
    }
}

fn segment_value(device_id: &str, prefix: &str) -> Option<String> {
    device_id
        .split('|')
        .find_map(|segment| segment.strip_prefix(prefix).map(str::to_string))
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

fn load_controller_profile_bindings() -> ControllerProfileBindings {
    let Ok(store) = load_store() else {
        return HashMap::new();
    };

    let mut bindings = HashMap::new();
    for profile in store.controllers {
        let Some(device_id) = profile.device_id else {
            continue;
        };

        let token_map = profile
            .mappings
            .into_iter()
            .filter_map(|(label, token)| {
                action_from_mapping_label(&label).map(|action| (token, action))
            })
            .collect::<HashMap<_, _>>();

        if !token_map.is_empty() {
            for alias in device_id_aliases(&device_id) {
                bindings.insert(alias, token_map.clone());
            }
        }
    }

    bindings
}

fn resolve_profile_action(
    bindings: &ControllerProfileBindings,
    device_id: &str,
    token: &str,
) -> Option<ConsoleAction> {
    device_id_aliases(device_id).into_iter().find_map(|alias| {
        bindings
            .get(&alias)
            .and_then(|token_map| token_map.get(token).copied())
    })
}

pub fn start_console_input(
    mut on_event: impl FnMut(ConsoleInputEvent) + Send + 'static,
) -> Result<ConsoleInputHandle, String> {
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_flag = std::sync::Arc::clone(&stop);

    let join_handle = thread::spawn(move || {
        if let Err(error) = run_console_input_loop(&mut on_event, stop_flag) {
            log_launch(&format!("SDL input loop stopped: {error}"));
        }
    });

    Ok(ConsoleInputHandle { stop, join_handle })
}

pub fn list_controller_input_devices() -> Vec<ControllerDeviceInfo> {
    list_controller_input_devices_impl()
}

pub fn list_all_controller_input_devices() -> Vec<ControllerDeviceInfo> {
    list_controller_input_devices_impl()
}

pub fn capture_controller_input_for_device(
    timeout: Duration,
    selected_device_id: Option<&str>,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    let sdl = sdl2::init().map_err(|error| format!("failed to initialize SDL: {error}"))?;
    let controller_subsystem = sdl
        .game_controller()
        .map_err(|error| format!("failed to initialize SDL game controller subsystem: {error}"))?;
    let joystick_subsystem = sdl
        .joystick()
        .map_err(|error| format!("failed to initialize SDL joystick subsystem: {error}"))?;
    let mut event_pump = sdl
        .event_pump()
        .map_err(|error| format!("failed to create SDL event pump: {error}"))?;

    let mut controllers = open_controllers(&controller_subsystem, &joystick_subsystem);
    let mut joysticks = open_generic_joysticks(&controller_subsystem, &joystick_subsystem);
    log_launch(&format!(
        "SDL capture: opened {} controller(s) and {} generic joystick(s)",
        controllers.len(),
        joysticks.len()
    ));

    while Instant::now() < deadline {
        for event in event_pump.poll_iter() {
            match event {
                Event::ControllerButtonDown { which, button, .. } => {
                    if let Some(controller) = controllers.get(&which) {
                        if !controller_matches_selected(controller, selected_device_id) {
                            continue;
                        }

                        if let Some(token) = controller_button_token(button) {
                            log_launch(&format!(
                                "SDL capture event: controller={} button={:?} token={token}",
                                controller.name(),
                                button,
                            ));
                            return Ok(token);
                        }
                    }
                }
                Event::JoyButtonDown {
                    which, button_idx, ..
                } => {
                    if let Some(joystick) = joysticks.get(&which) {
                        if !joystick_matches_selected(joystick, selected_device_id) {
                            continue;
                        }

                        let token = joystick_button_token(button_idx);
                        log_launch(&format!(
                            "SDL capture event: joystick={} button={} token={token}",
                            joystick.joystick.name(),
                            button_idx,
                        ));
                        return Ok(token);
                    }
                }
                Event::JoyAxisMotion {
                    which,
                    axis_idx,
                    value,
                    ..
                } => {
                    if let Some(joystick) = joysticks.get_mut(&which) {
                        if !joystick_matches_selected(joystick, selected_device_id) {
                            continue;
                        }

                        if should_ignore_startup_axis_motion(joystick, axis_idx, value) {
                            continue;
                        }

                        if let Some(token) = joystick_axis_token(axis_idx, value) {
                            log_launch(&format!(
                                "SDL capture event: joystick={} axis={} value={} token={token}",
                                joystick.joystick.name(),
                                axis_idx,
                                value,
                            ));
                            return Ok(token);
                        }
                    }
                }
                Event::ControllerAxisMotion {
                    which, axis, value, ..
                } => {
                    if let Some(controller) = controllers.get(&which) {
                        if !controller_matches_selected(controller, selected_device_id) {
                            continue;
                        }

                        if let Some(token) = controller_axis_token(axis, value) {
                            log_launch(&format!(
                                "SDL capture event: controller={} axis={:?} value={} token={token}",
                                controller.name(),
                                axis,
                                value,
                            ));
                            return Ok(token);
                        }
                    }
                }
                Event::KeyDown {
                    keycode: Some(keycode),
                    repeat: false,
                    ..
                } => {
                    if let Some(token) = keyboard_token(keycode) {
                        log_launch(&format!(
                            "SDL capture event: keyboard={keycode:?} token={token}"
                        ));
                        return Ok(token);
                    }
                }
                Event::JoyDeviceAdded { which, .. } => {
                    if !controller_subsystem.is_game_controller(which) {
                        if let Ok(joystick) = joystick_subsystem.open(which) {
                            let instance_id = joystick.instance_id();
                            if joysticks.contains_key(&instance_id) {
                                log_launch(&format!(
                                    "SDL capture: ignoring duplicate startup joystick add for instance={instance_id}"
                                ));
                                continue;
                            }
                            let joystick_name = joystick.name();
                            let device_id = joystick_unique_id(&joystick);
                            let pending_neutral_axes = initial_pending_neutral_axes(&joystick);
                            let axis_input_ready = pending_neutral_axes.is_empty();
                            log_launch(&format!(
                                "SDL capture joystick connected: instance={} name={}",
                                instance_id, joystick_name
                            ));
                            joysticks.insert(
                                instance_id,
                                OpenJoystick {
                                    joystick,
                                    device_id,
                                    pending_neutral_axes,
                                    axis_input_ready,
                                },
                            );
                        }
                    }
                }
                Event::JoyDeviceRemoved { which, .. } => {
                    joysticks.remove(&which);
                    log_launch(&format!("SDL capture joystick disconnected: {which}"));
                }
                Event::ControllerDeviceAdded { which, .. } => {
                    if let Ok(controller) = controller_subsystem.open(which) {
                        let controller_name = controller.name();
                        log_launch(&format!(
                            "SDL capture controller connected: {controller_name}"
                        ));
                        controllers.insert(controller.instance_id(), controller);
                    }
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    controllers.remove(&which);
                    log_launch(&format!("SDL capture controller disconnected: {which}"));
                }
                _ => {}
            }
        }

        thread::sleep(POLL_INTERVAL);
    }

    Err("timeout waiting for SDL controller input".to_string())
}

fn run_console_input_loop(
    on_event: &mut impl FnMut(ConsoleInputEvent),
    stop: std::sync::Arc<AtomicBool>,
) -> Result<(), String> {
    let sdl = sdl2::init().map_err(|error| format!("failed to initialize SDL: {error}"))?;
    let controller_subsystem = sdl
        .game_controller()
        .map_err(|error| format!("failed to initialize SDL game controller subsystem: {error}"))?;
    let joystick_subsystem = sdl
        .joystick()
        .map_err(|error| format!("failed to initialize SDL joystick subsystem: {error}"))?;
    let mut event_pump = sdl
        .event_pump()
        .map_err(|error| format!("failed to create SDL event pump: {error}"))?;

    maybe_load_controller_mappings(&controller_subsystem);

    let mut controllers = open_controllers(&controller_subsystem, &joystick_subsystem);
    let mut joysticks = open_generic_joysticks(&controller_subsystem, &joystick_subsystem);
    replace_runtime_controller_devices(collect_runtime_controller_devices(
        &controllers,
        &joysticks,
    ));
    let mut keyboard_backend = KeyboardBackend::new();
    let mut profile_bindings = load_controller_profile_bindings();
    let mut profile_revision = controller_store_revision();
    let mut next_profile_reload_at = Instant::now() + Duration::from_secs(1);
    let mut action_states: HashMap<ConsoleAction, ActionState> = HashMap::new();
    let mut unmapped_controller_sources: HashSet<SourceId> = HashSet::new();
    let mut debug_events_logged = 0usize;
    let mut repeat_debug_events_logged = 0usize;

    log_launch("keyboard backend build marker: raw-proc-input-v3");
    let keyboard_candidates = keyboard_device_candidates();
    log_launch(&format!(
        "keyboard backend candidates: count={} devices={:?}",
        keyboard_candidates.len(),
        keyboard_candidates
    ));
    let controller_names = controllers
        .values()
        .map(|controller| controller.name())
        .collect::<Vec<_>>();
    let joystick_names = joysticks
        .values()
        .map(|joystick| joystick.joystick.name())
        .collect::<Vec<_>>();
    log_launch(&format!(
        "SDL input v4: active controller count={} names={:?} generic joystick count={} names={:?} keyboard candidates={}",
        controllers.len(),
        controller_names,
        joysticks.len(),
        joystick_names,
        keyboard_candidates.len()
    ));

    while !stop.load(Ordering::Relaxed) {
        let loop_now = Instant::now();
        let current_profile_revision = controller_store_revision();
        if current_profile_revision != profile_revision || loop_now >= next_profile_reload_at {
            profile_bindings = load_controller_profile_bindings();
            if current_profile_revision != profile_revision {
                clear_controller_action_states(&mut action_states, on_event, 0);
                unmapped_controller_sources.clear();
                profile_revision = current_profile_revision;
            }
            next_profile_reload_at = loop_now + Duration::from_secs(1);
        }

        for keyboard_event in keyboard_backend.poll_events() {
            let timestamp = match &keyboard_event {
                RawKeyboardEvent::Connected { .. } | RawKeyboardEvent::Disconnected { .. } => 0,
                RawKeyboardEvent::Key { timestamp, .. } => *timestamp,
            };

            match keyboard_event {
                RawKeyboardEvent::Connected {
                    device_node,
                    device_name,
                } => {
                    log_launch(&format!(
                        "keyboard input: connected node={device_node} name={device_name}"
                    ));
                    on_event(ConsoleInputEvent {
                        phase: ConsoleInputPhase::Connected,
                        source: ConsoleInputSource::Keyboard {
                            keycode: String::new(),
                            device_name: device_name.clone(),
                        },
                        action: None,
                        token: format!("KEYBOARD_CONNECTED:{device_node}"),
                        pressed: false,
                        timestamp,
                        controller_name: device_name,
                    });
                }
                RawKeyboardEvent::Disconnected {
                    device_node,
                    device_name,
                } => {
                    log_launch(&format!(
                        "keyboard input: disconnected node={device_node} name={device_name}"
                    ));
                    on_event(ConsoleInputEvent {
                        phase: ConsoleInputPhase::Disconnected,
                        source: ConsoleInputSource::Keyboard {
                            keycode: String::new(),
                            device_name: device_name.clone(),
                        },
                        action: None,
                        token: format!("KEYBOARD_DISCONNECTED:{device_node}"),
                        pressed: false,
                        timestamp,
                        controller_name: device_name,
                    });
                }
                RawKeyboardEvent::Key {
                    device_node,
                    device_name,
                    key_code,
                    value,
                    timestamp,
                } => {
                    let key_name = key_code_name(key_code).to_string();
                    let token = format!("Keyboard:{key_name}");
                    if let Some(action) = raw_keyboard_action(key_code) {
                        let source = SourceId::Keyboard {
                            device_node: device_node.clone(),
                            key_code,
                        };
                        if value == 1 {
                            if push_action(
                                &mut action_states,
                                action,
                                source,
                                DEFAULT_INITIAL_REPEAT_DELAY,
                            ) {
                                if debug_events_logged < 128 {
                                    log_launch(&format!("keyboard input: node={device_node} name={device_name} code={key_name} value={value} action={} pressed=1 token={token}", action.as_str()));
                                    debug_events_logged += 1;
                                }
                                on_event(ConsoleInputEvent {
                                    phase: ConsoleInputPhase::Pressed,
                                    source: ConsoleInputSource::Keyboard {
                                        keycode: key_name.clone(),
                                        device_name: device_name.clone(),
                                    },
                                    action: Some(action),
                                    token,
                                    pressed: true,
                                    timestamp,
                                    controller_name: device_name.clone(),
                                });
                            }
                        } else if value == 0 {
                            if release_action(&mut action_states, action, source) {
                                if debug_events_logged < 128 {
                                    log_launch(&format!("keyboard input: node={device_node} name={device_name} code={key_name} value={value} action={} pressed=0 token={token}", action.as_str()));
                                    debug_events_logged += 1;
                                }
                                on_event(ConsoleInputEvent {
                                    phase: ConsoleInputPhase::Released,
                                    source: ConsoleInputSource::Keyboard {
                                        keycode: key_name.clone(),
                                        device_name: device_name.clone(),
                                    },
                                    action: Some(action),
                                    token,
                                    pressed: false,
                                    timestamp,
                                    controller_name: device_name.clone(),
                                });
                            }
                        } else if debug_events_logged < 128 {
                            log_launch(&format!("keyboard input: node={device_node} name={device_name} code={key_name} value={value} action={} repeat/ignored token={token}", action.as_str()));
                            debug_events_logged += 1;
                        }
                    } else if debug_events_logged < 128 {
                        log_launch(&format!("keyboard input: node={device_node} name={device_name} code={key_name} value={value} ignored token={token}"));
                        debug_events_logged += 1;
                    }
                }
            }
        }

        for event in event_pump.poll_iter() {
            let timestamp = event.get_timestamp();
            match event {
                Event::JoyDeviceAdded { which, .. } => {
                    if !controller_subsystem.is_game_controller(which) {
                        match joystick_subsystem.open(which) {
                            Ok(joystick) => {
                                let instance_id = joystick.instance_id();
                                if joysticks.contains_key(&instance_id) {
                                    log_launch(&format!(
                                        "SDL input: ignoring duplicate startup joystick add for instance={instance_id}"
                                    ));
                                    continue;
                                }
                                let name = joystick.name();
                                let device_id = joystick_unique_id(&joystick);
                                let pending_neutral_axes = initial_pending_neutral_axes(&joystick);
                                let axis_input_ready = pending_neutral_axes.is_empty();
                                log_launch(&format!(
                                    "SDL input: joystick connected instance={instance_id} name={name}"
                                ));
                                if !pending_neutral_axes.is_empty() {
                                    log_launch(&format!(
                                        "SDL input: joystick instance={} waiting for neutral after connect axes {:?}",
                                        instance_id, pending_neutral_axes
                                    ));
                                }
                                on_event(ConsoleInputEvent {
                                    phase: ConsoleInputPhase::Connected,
                                    source: ConsoleInputSource::Controller {
                                        instance_id,
                                        device_id: device_id.clone(),
                                        controller_name: name.clone(),
                                    },
                                    action: None,
                                    token: format!("SDL_JOYSTICK_CONNECTED:{instance_id}"),
                                    pressed: false,
                                    timestamp,
                                    controller_name: name.clone(),
                                });
                                joysticks.insert(
                                    instance_id,
                                    OpenJoystick {
                                        joystick,
                                        device_id,
                                        pending_neutral_axes,
                                        axis_input_ready,
                                    },
                                );
                                replace_runtime_controller_devices(
                                    collect_runtime_controller_devices(&controllers, &joysticks),
                                );
                            }
                            Err(error) => {
                                log_launch(&format!(
                                    "SDL input: failed to open joystick {}: {error}",
                                    which
                                ));
                            }
                        }
                    }
                }
                Event::JoyDeviceRemoved { which, .. } => {
                    if let Some(joystick) = joysticks.remove(&which) {
                        replace_runtime_controller_devices(collect_runtime_controller_devices(
                            &controllers,
                            &joysticks,
                        ));
                        let controller_name = joystick.joystick.name();
                        remove_controller_from_states(
                            &mut action_states,
                            which,
                            on_event,
                            timestamp,
                            &controller_name,
                        );
                        log_launch(&format!("SDL input: joystick disconnected instance={which} name={controller_name}"));
                        on_event(ConsoleInputEvent {
                            phase: ConsoleInputPhase::Disconnected,
                            source: ConsoleInputSource::Controller {
                                instance_id: which,
                                device_id: joystick.device_id,
                                controller_name: controller_name.clone(),
                            },
                            action: None,
                            token: format!("SDL_JOYSTICK_DISCONNECTED:{which}"),
                            pressed: false,
                            timestamp,
                            controller_name,
                        });
                    }
                }
                Event::ControllerDeviceAdded { which, .. } => {
                    match controller_subsystem.open(which) {
                        Ok(controller) => {
                            let instance_id = controller.instance_id();
                            if controllers.contains_key(&instance_id) {
                                log_launch(&format!(
                                    "SDL input: ignoring duplicate startup controller add for instance={instance_id}"
                                ));
                                continue;
                            }
                            let name = controller.name();
                            log_launch(&format!("SDL input: controller connected instance={instance_id} name={name}"));
                            on_event(ConsoleInputEvent {
                                phase: ConsoleInputPhase::Connected,
                                source: ConsoleInputSource::Controller {
                                    instance_id,
                                    device_id: controller_unique_id(&controller),
                                    controller_name: name.clone(),
                                },
                                action: None,
                                token: format!("SDL_CONTROLLER_CONNECTED:{instance_id}"),
                                pressed: false,
                                timestamp,
                                controller_name: name.clone(),
                            });
                            controllers.insert(instance_id, controller);
                            replace_runtime_controller_devices(collect_runtime_controller_devices(
                                &controllers,
                                &joysticks,
                            ));
                        }
                        Err(error) => {
                            log_launch(&format!(
                                "SDL input: failed to open controller {}: {error}",
                                which
                            ));
                        }
                    }
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    if let Some(controller) = controllers.remove(&which) {
                        replace_runtime_controller_devices(collect_runtime_controller_devices(
                            &controllers,
                            &joysticks,
                        ));
                        let controller_name = controller.name();
                        remove_controller_from_states(
                            &mut action_states,
                            which,
                            on_event,
                            timestamp,
                            &controller_name,
                        );
                        log_launch(&format!("SDL input: controller disconnected instance={which} name={controller_name}"));
                        on_event(ConsoleInputEvent {
                            phase: ConsoleInputPhase::Disconnected,
                            source: ConsoleInputSource::Controller {
                                instance_id: which,
                                device_id: controller_unique_id(&controller),
                                controller_name: controller_name.clone(),
                            },
                            action: None,
                            token: format!("SDL_CONTROLLER_DISCONNECTED:{which}"),
                            pressed: false,
                            timestamp,
                            controller_name,
                        });
                    }
                }
                Event::ControllerButtonDown { which, button, .. } => {
                    if let Some(controller) = controllers.get(&which) {
                        let controller_name = controller.name();
                        let source = SourceId::ControllerButton(which, button);
                        let token = controller_button_token(button)
                            .unwrap_or_else(|| format!("SDL_CONTROLLER_BUTTON_{button:?}"));
                        let device_id = controller_unique_id(controller);
                        if let Some(action) = resolve_profile_action(&profile_bindings, &device_id, &token) {
                            if push_action(
                                &mut action_states,
                                action,
                                source,
                                DEFAULT_INITIAL_REPEAT_DELAY,
                            ) {
                                if debug_events_logged < 64 {
                                    log_launch(&format!("SDL input: controller={} button={button:?} action={} pressed=1 token={token}", controller_name, action.as_str()));
                                    debug_events_logged += 1;
                                }
                                on_event(ConsoleInputEvent {
                                    phase: ConsoleInputPhase::Pressed,
                                    source: ConsoleInputSource::Controller {
                                        instance_id: which,
                                        device_id,
                                        controller_name: controller_name.clone(),
                                    },
                                    action: Some(action),
                                    token,
                                    pressed: true,
                                    timestamp,
                                    controller_name,
                                });
                            }
                        } else if unmapped_controller_sources.insert(source.clone()) {
                            emit_unmapped_pressed_event(
                                on_event,
                                which,
                                &device_id,
                                &controller_name,
                                &token,
                                timestamp,
                            );
                        }
                    }
                }
                Event::ControllerButtonUp { which, button, .. } => {
                    if let Some(controller) = controllers.get(&which) {
                        let controller_name = controller.name();
                        let source = SourceId::ControllerButton(which, button);
                        let token = controller_button_token(button)
                            .unwrap_or_else(|| format!("SDL_CONTROLLER_BUTTON_{button:?}"));
                        let device_id = controller_unique_id(controller);
                        if let Some(action) = resolve_profile_action(&profile_bindings, &device_id, &token) {
                            if release_action(&mut action_states, action, source) {
                                if debug_events_logged < 64 {
                                    log_launch(&format!("SDL input: controller={} button={button:?} action={} pressed=0 token={token}", controller_name, action.as_str()));
                                    debug_events_logged += 1;
                                }
                                on_event(ConsoleInputEvent {
                                    phase: ConsoleInputPhase::Released,
                                    source: ConsoleInputSource::Controller {
                                        instance_id: which,
                                        device_id,
                                        controller_name: controller_name.clone(),
                                    },
                                    action: Some(action),
                                    token,
                                    pressed: false,
                                    timestamp,
                                    controller_name,
                                });
                            }
                        } else if unmapped_controller_sources.remove(&source) {
                            emit_unmapped_released_event(
                                on_event,
                                which,
                                &device_id,
                                &controller_name,
                                &token,
                                timestamp,
                            );
                        }
                    }
                }
                Event::ControllerAxisMotion {
                    which, axis, value, ..
                } => {
                    if let Some(controller) = controllers.get(&which) {
                        handle_controller_axis_event(
                            &mut action_states,
                            &mut unmapped_controller_sources,
                            which,
                            axis,
                            value,
                            timestamp,
                            &profile_bindings,
                            controller_unique_id(controller),
                            controller.name(),
                            on_event,
                            &mut debug_events_logged,
                        );
                    }
                }
                Event::JoyButtonDown {
                    which, button_idx, ..
                } => {
                    if let Some(joystick) = joysticks.get(&which) {
                        let controller_name = joystick.joystick.name();
                        let source = SourceId::JoystickButton {
                            instance_id: which,
                            button_idx,
                        };
                        let token = joystick_button_token(button_idx);
                        if let Some(action) = resolve_profile_action(&profile_bindings, &joystick.device_id, &token) {
                            if push_action(
                                &mut action_states,
                                action,
                                source,
                                DEFAULT_INITIAL_REPEAT_DELAY,
                            ) {
                                if debug_events_logged < 64 {
                                    log_launch(&format!("SDL input: joystick={} button={} action={} pressed=1 token={token}", controller_name, button_idx, action.as_str()));
                                    debug_events_logged += 1;
                                }
                                on_event(ConsoleInputEvent {
                                    phase: ConsoleInputPhase::Pressed,
                                    source: ConsoleInputSource::Controller {
                                        instance_id: which,
                                        device_id: joystick.device_id.clone(),
                                        controller_name: controller_name.clone(),
                                    },
                                    action: Some(action),
                                    token,
                                    pressed: true,
                                    timestamp,
                                    controller_name,
                                });
                            }
                        } else if unmapped_controller_sources.insert(source.clone()) {
                            emit_unmapped_pressed_event(
                                on_event,
                                which,
                                &joystick.device_id,
                                &controller_name,
                                &token,
                                timestamp,
                            );
                        }
                    }
                }
                Event::JoyButtonUp {
                    which, button_idx, ..
                } => {
                    if let Some(joystick) = joysticks.get(&which) {
                        let controller_name = joystick.joystick.name();
                        let source = SourceId::JoystickButton {
                            instance_id: which,
                            button_idx,
                        };
                        let token = joystick_button_token(button_idx);
                        if let Some(action) = resolve_profile_action(&profile_bindings, &joystick.device_id, &token) {
                            if release_action(&mut action_states, action, source) {
                                if debug_events_logged < 64 {
                                    log_launch(&format!("SDL input: joystick={} button={} action={} pressed=0 token={token}", controller_name, button_idx, action.as_str()));
                                    debug_events_logged += 1;
                                }
                                on_event(ConsoleInputEvent {
                                    phase: ConsoleInputPhase::Released,
                                    source: ConsoleInputSource::Controller {
                                        instance_id: which,
                                        device_id: joystick.device_id.clone(),
                                        controller_name: controller_name.clone(),
                                    },
                                    action: Some(action),
                                    token,
                                    pressed: false,
                                    timestamp,
                                    controller_name,
                                });
                            }
                        } else if unmapped_controller_sources.remove(&source) {
                            emit_unmapped_released_event(
                                on_event,
                                which,
                                &joystick.device_id,
                                &controller_name,
                                &token,
                                timestamp,
                            );
                        }
                    }
                }
                Event::JoyAxisMotion {
                    which,
                    axis_idx,
                    value,
                    ..
                } => {
                    if let Some(joystick) = joysticks.get_mut(&which) {
                        if should_ignore_startup_axis_motion(joystick, axis_idx, value) {
                            continue;
                        }

                        handle_joystick_axis_event(
                            &mut action_states,
                            &mut unmapped_controller_sources,
                            which,
                            axis_idx,
                            value,
                            timestamp,
                            &profile_bindings,
                            joystick.device_id.clone(),
                            joystick.joystick.name(),
                            on_event,
                            &mut debug_events_logged,
                        );
                    }
                }
                _ => {}
            }
        }

        reconcile_joystick_axis_states(
            &mut action_states,
            &mut unmapped_controller_sources,
            &mut joysticks,
            &profile_bindings,
            on_event,
            &mut debug_events_logged,
        );

        let now = Instant::now();
        for (action, state) in action_states.iter_mut() {
            if !state.active_sources.is_empty() {
                if state.next_repeat_at.is_none() {
                    state.next_repeat_at = Some(now + repeat_initial_delay());
                }

                if let Some(next_repeat_at) = state.next_repeat_at {
                    if now >= next_repeat_at {
                        state.next_repeat_at = Some(now + repeat_interval());
                        if repeat_debug_events_logged < 64 {
                            log_launch(&format!(
                                "input repeat: action={} sources=[{}]",
                                action.as_str(),
                                describe_sources(&state.active_sources)
                            ));
                            repeat_debug_events_logged += 1;
                        }
                        on_event(ConsoleInputEvent {
                            phase: ConsoleInputPhase::Repeat,
                            source: ConsoleInputSource::Keyboard {
                                keycode: "repeat".to_string(),
                                device_name: String::new(),
                            },
                            action: Some(*action),
                            token: format!("REPEAT:{}", action.as_str()),
                            pressed: true,
                            timestamp: now.elapsed().as_millis().min(u32::MAX as u128) as u32,
                            controller_name: String::new(),
                        });
                    }
                }
            }
        }

        thread::sleep(POLL_INTERVAL);
    }

    Ok(())
}

fn repeat_initial_delay() -> Duration {
    env::var("OPENCONSOLE_INPUT_REPEAT_INITIAL_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_INITIAL_REPEAT_DELAY)
}

fn repeat_interval() -> Duration {
    env::var("OPENCONSOLE_INPUT_REPEAT_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_REPEAT_INTERVAL)
}

fn raw_keyboard_action(key_code: u16) -> Option<ConsoleAction> {
    match key_code {
        crate::keyboard_input::KEY_UP | crate::keyboard_input::KEY_W => Some(ConsoleAction::Up),
        crate::keyboard_input::KEY_DOWN | crate::keyboard_input::KEY_S => Some(ConsoleAction::Down),
        crate::keyboard_input::KEY_LEFT | crate::keyboard_input::KEY_A => Some(ConsoleAction::Left),
        crate::keyboard_input::KEY_RIGHT | crate::keyboard_input::KEY_D => {
            Some(ConsoleAction::Right)
        }
        crate::keyboard_input::KEY_SPACE | crate::keyboard_input::KEY_ENTER => {
            Some(ConsoleAction::Accept)
        }
        crate::keyboard_input::KEY_ESC => Some(ConsoleAction::Cancel),
        crate::keyboard_input::KEY_TAB => Some(ConsoleAction::Select),
        _ => None,
    }
}

fn push_action(
    action_states: &mut HashMap<ConsoleAction, ActionState>,
    action: ConsoleAction,
    source: SourceId,
    repeat_delay: Duration,
) -> bool {
    let state = action_states.entry(action).or_default();
    let was_empty = state.active_sources.is_empty();
    if state.active_sources.insert(source) && was_empty {
        state.next_repeat_at = Some(Instant::now() + repeat_delay);
        return true;
    }

    false
}

fn release_action(
    action_states: &mut HashMap<ConsoleAction, ActionState>,
    action: ConsoleAction,
    source: SourceId,
) -> bool {
    let Some(state) = action_states.get_mut(&action) else {
        return false;
    };

    if !state.active_sources.remove(&source) {
        return false;
    }

    if state.active_sources.is_empty() {
        state.next_repeat_at = None;
        return true;
    }

    false
}

fn remove_controller_from_states(
    action_states: &mut HashMap<ConsoleAction, ActionState>,
    controller_instance_id: u32,
    on_event: &mut impl FnMut(ConsoleInputEvent),
    timestamp: u32,
    controller_name: &str,
) {
    for (action, state) in action_states.iter_mut() {
        let mut removed_any = false;
        state.active_sources.retain(|source| match source {
            SourceId::ControllerButton(instance_id, _)
            | SourceId::ControllerAxis { instance_id, .. }
            | SourceId::JoystickButton { instance_id, .. }
            | SourceId::JoystickAxis { instance_id, .. }
                if *instance_id == controller_instance_id =>
            {
                removed_any = true;
                false
            }
            _ => true,
        });

        if removed_any && state.active_sources.is_empty() {
            state.next_repeat_at = None;
            on_event(ConsoleInputEvent {
                phase: ConsoleInputPhase::Released,
                source: ConsoleInputSource::Controller {
                    instance_id: controller_instance_id,
                    device_id: String::new(),
                    controller_name: controller_name.to_string(),
                },
                action: Some(*action),
                token: format!("RELEASED:{}", action.as_str()),
                pressed: false,
                timestamp,
                controller_name: controller_name.to_string(),
            });
        }
    }
}

fn clear_controller_action_states(
    action_states: &mut HashMap<ConsoleAction, ActionState>,
    on_event: &mut impl FnMut(ConsoleInputEvent),
    timestamp: u32,
) {
    for (action, state) in action_states.iter_mut() {
        let mut removed_any = false;
        state.active_sources.retain(|source| {
            let keep = matches!(source, SourceId::Keyboard { .. });
            if !keep {
                removed_any = true;
            }
            keep
        });

        if removed_any && state.active_sources.is_empty() {
            state.next_repeat_at = None;
            on_event(ConsoleInputEvent {
                phase: ConsoleInputPhase::Released,
                source: ConsoleInputSource::Controller {
                    instance_id: 0,
                    device_id: String::new(),
                    controller_name: String::new(),
                },
                action: Some(*action),
                token: format!("RELEASED:{}", action.as_str()),
                pressed: false,
                timestamp,
                controller_name: String::new(),
            });
        }
    }
}

fn keyboard_token(keycode: Keycode) -> Option<String> {
    Some(match keycode {
        Keycode::Up => "Keyboard:Up".to_string(),
        Keycode::Down => "Keyboard:Down".to_string(),
        Keycode::Left => "Keyboard:Left".to_string(),
        Keycode::Right => "Keyboard:Right".to_string(),
        Keycode::W => "Keyboard:W".to_string(),
        Keycode::A => "Keyboard:A".to_string(),
        Keycode::S => "Keyboard:S".to_string(),
        Keycode::D => "Keyboard:D".to_string(),
        Keycode::Space => "Keyboard:Space".to_string(),
        Keycode::Return | Keycode::KpEnter | Keycode::Return2 => "Keyboard:Return".to_string(),
        Keycode::Escape => "Keyboard:Escape".to_string(),
        Keycode::Tab => "Keyboard:Tab".to_string(),
        _ => return None,
    })
}

fn controller_button_token(button: Button) -> Option<String> {
    Some(match button {
        Button::A => "SDL_CONTROLLER_BUTTON_A".to_string(),
        Button::B => "SDL_CONTROLLER_BUTTON_B".to_string(),
        Button::X => "SDL_CONTROLLER_BUTTON_X".to_string(),
        Button::Y => "SDL_CONTROLLER_BUTTON_Y".to_string(),
        Button::Back => "SDL_CONTROLLER_BUTTON_BACK".to_string(),
        Button::Start => "SDL_CONTROLLER_BUTTON_START".to_string(),
        Button::LeftStick => "SDL_CONTROLLER_BUTTON_LEFTSTICK".to_string(),
        Button::RightStick => "SDL_CONTROLLER_BUTTON_RIGHTSTICK".to_string(),
        Button::LeftShoulder => "SDL_CONTROLLER_BUTTON_LEFTSHOULDER".to_string(),
        Button::RightShoulder => "SDL_CONTROLLER_BUTTON_RIGHTSHOULDER".to_string(),
        Button::DPadUp => "SDL_CONTROLLER_BUTTON_DPAD_UP".to_string(),
        Button::DPadDown => "SDL_CONTROLLER_BUTTON_DPAD_DOWN".to_string(),
        Button::DPadLeft => "SDL_CONTROLLER_BUTTON_DPAD_LEFT".to_string(),
        Button::DPadRight => "SDL_CONTROLLER_BUTTON_DPAD_RIGHT".to_string(),
        Button::Guide => "SDL_CONTROLLER_BUTTON_GUIDE".to_string(),
        Button::Misc1 => "SDL_CONTROLLER_BUTTON_MISC1".to_string(),
        Button::Paddle1 => "SDL_CONTROLLER_BUTTON_PADDLE1".to_string(),
        Button::Paddle2 => "SDL_CONTROLLER_BUTTON_PADDLE2".to_string(),
        Button::Paddle3 => "SDL_CONTROLLER_BUTTON_PADDLE3".to_string(),
        Button::Paddle4 => "SDL_CONTROLLER_BUTTON_PADDLE4".to_string(),
        Button::Touchpad => "SDL_CONTROLLER_BUTTON_TOUCHPAD".to_string(),
    })
}

fn joystick_button_token(button_idx: u8) -> String {
    format!("SDL_JOYSTICK_BUTTON_{button_idx}")
}

fn joystick_axis_token(axis_idx: u8, value: i16) -> Option<String> {
    if value <= -DEAD_ZONE {
        Some(format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"))
    } else if value >= DEAD_ZONE {
        Some(format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"))
    } else {
        None
    }
}

fn controller_axis_token(axis: Axis, value: i16) -> Option<String> {
    match axis {
        Axis::LeftX | Axis::LeftY => {
            if value <= -DEAD_ZONE {
                Some(format!("SDL_CONTROLLER_AXIS_{axis:?}_NEG"))
            } else if value >= DEAD_ZONE {
                Some(format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"))
            } else {
                None
            }
        }
        Axis::TriggerLeft | Axis::TriggerRight => {
            if value >= DEAD_ZONE {
                Some(format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn emit_pressed_event(
    on_event: &mut impl FnMut(ConsoleInputEvent),
    instance_id: u32,
    device_id: &str,
    controller_name: &str,
    action: ConsoleAction,
    token: &str,
    timestamp: u32,
) {
    on_event(ConsoleInputEvent {
        phase: ConsoleInputPhase::Pressed,
        source: ConsoleInputSource::Controller {
            instance_id,
            device_id: device_id.to_string(),
            controller_name: controller_name.to_string(),
        },
        action: Some(action),
        token: token.to_string(),
        pressed: true,
        timestamp,
        controller_name: controller_name.to_string(),
    });
}

fn emit_unmapped_pressed_event(
    on_event: &mut impl FnMut(ConsoleInputEvent),
    instance_id: u32,
    device_id: &str,
    controller_name: &str,
    token: &str,
    timestamp: u32,
) {
    on_event(ConsoleInputEvent {
        phase: ConsoleInputPhase::Pressed,
        source: ConsoleInputSource::Controller {
            instance_id,
            device_id: device_id.to_string(),
            controller_name: controller_name.to_string(),
        },
        action: None,
        token: token.to_string(),
        pressed: true,
        timestamp,
        controller_name: controller_name.to_string(),
    });
}

fn emit_released_event(
    on_event: &mut impl FnMut(ConsoleInputEvent),
    instance_id: u32,
    device_id: &str,
    controller_name: &str,
    action: ConsoleAction,
    token: &str,
    timestamp: u32,
) {
    on_event(ConsoleInputEvent {
        phase: ConsoleInputPhase::Released,
        source: ConsoleInputSource::Controller {
            instance_id,
            device_id: device_id.to_string(),
            controller_name: controller_name.to_string(),
        },
        action: Some(action),
        token: token.to_string(),
        pressed: false,
        timestamp,
        controller_name: controller_name.to_string(),
    });
}

fn emit_unmapped_released_event(
    on_event: &mut impl FnMut(ConsoleInputEvent),
    instance_id: u32,
    device_id: &str,
    controller_name: &str,
    token: &str,
    timestamp: u32,
) {
    on_event(ConsoleInputEvent {
        phase: ConsoleInputPhase::Released,
        source: ConsoleInputSource::Controller {
            instance_id,
            device_id: device_id.to_string(),
            controller_name: controller_name.to_string(),
        },
        action: None,
        token: token.to_string(),
        pressed: false,
        timestamp,
        controller_name: controller_name.to_string(),
    });
}

fn handle_direction_transition(
    action_states: &mut HashMap<ConsoleAction, ActionState>,
    unmapped_controller_sources: &mut HashSet<SourceId>,
    negative_action: Option<ConsoleAction>,
    negative_source: SourceId,
    negative_token: String,
    positive_action: Option<ConsoleAction>,
    positive_source: SourceId,
    positive_token: String,
    direction: i8,
    repeat_delay: Duration,
    instance_id: u32,
    device_id: &str,
    controller_name: &str,
    timestamp: u32,
    on_event: &mut impl FnMut(ConsoleInputEvent),
    debug_events_logged: &mut usize,
    debug_prefix: &str,
) {
    if let Some(negative_action) = negative_action {
        if direction != -1
            && release_action(action_states, negative_action, negative_source.clone())
        {
            if *debug_events_logged < 64 {
                log_launch(&format!(
                    "{debug_prefix} action={} pressed=0 token={negative_token}",
                    negative_action.as_str()
                ));
                *debug_events_logged += 1;
            }
            emit_released_event(
                on_event,
                instance_id,
                device_id,
                controller_name,
                negative_action,
                &negative_token,
                timestamp,
            );
        }
    } else if direction != -1 && unmapped_controller_sources.remove(&negative_source) {
        emit_unmapped_released_event(
            on_event,
            instance_id,
            device_id,
            controller_name,
            &negative_token,
            timestamp,
        );
    }

    if let Some(positive_action) = positive_action {
        if direction != 1 && release_action(action_states, positive_action, positive_source.clone())
        {
            if *debug_events_logged < 64 {
                log_launch(&format!(
                    "{debug_prefix} action={} pressed=0 token={positive_token}",
                    positive_action.as_str()
                ));
                *debug_events_logged += 1;
            }
            emit_released_event(
                on_event,
                instance_id,
                device_id,
                controller_name,
                positive_action,
                &positive_token,
                timestamp,
            );
        }
    } else if direction != 1 && unmapped_controller_sources.remove(&positive_source) {
        emit_unmapped_released_event(
            on_event,
            instance_id,
            device_id,
            controller_name,
            &positive_token,
            timestamp,
        );
    }

    if let Some(negative_action) = negative_action {
        if direction == -1
            && push_action(
                action_states,
                negative_action,
                negative_source,
                repeat_delay,
            )
        {
            if *debug_events_logged < 64 {
                log_launch(&format!(
                    "{debug_prefix} action={} pressed=1 token={negative_token}",
                    negative_action.as_str()
                ));
                *debug_events_logged += 1;
            }
            emit_pressed_event(
                on_event,
                instance_id,
                device_id,
                controller_name,
                negative_action,
                &negative_token,
                timestamp,
            );
        }
    } else if direction == -1 && unmapped_controller_sources.insert(negative_source) {
        emit_unmapped_pressed_event(
            on_event,
            instance_id,
            device_id,
            controller_name,
            &negative_token,
            timestamp,
        );
    }

    if let Some(positive_action) = positive_action {
        if direction == 1
            && push_action(
                action_states,
                positive_action,
                positive_source,
                repeat_delay,
            )
        {
            if *debug_events_logged < 64 {
                log_launch(&format!(
                    "{debug_prefix} action={} pressed=1 token={positive_token}",
                    positive_action.as_str()
                ));
                *debug_events_logged += 1;
            }
            emit_pressed_event(
                on_event,
                instance_id,
                device_id,
                controller_name,
                positive_action,
                &positive_token,
                timestamp,
            );
        }
    } else if direction == 1 && unmapped_controller_sources.insert(positive_source) {
        emit_unmapped_pressed_event(
            on_event,
            instance_id,
            device_id,
            controller_name,
            &positive_token,
            timestamp,
        );
    }
}

fn handle_controller_axis_event(
    action_states: &mut HashMap<ConsoleAction, ActionState>,
    unmapped_controller_sources: &mut HashSet<SourceId>,
    instance_id: u32,
    axis: Axis,
    value: i16,
    timestamp: u32,
    profile_bindings: &ControllerProfileBindings,
    device_id: String,
    controller_name: String,
    on_event: &mut impl FnMut(ConsoleInputEvent),
    debug_events_logged: &mut usize,
) {
    match axis {
        Axis::LeftX => handle_direction_transition(
            action_states,
            unmapped_controller_sources,
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_CONTROLLER_AXIS_{axis:?}_NEG"),
            ),
            SourceId::ControllerAxis {
                instance_id,
                axis,
                direction: -1,
            },
            format!("SDL_CONTROLLER_AXIS_{axis:?}_NEG"),
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"),
            ),
            SourceId::ControllerAxis {
                instance_id,
                axis,
                direction: 1,
            },
            format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"),
            axis_direction(value),
            DEFAULT_INITIAL_REPEAT_DELAY,
            instance_id,
            &device_id,
            &controller_name,
            timestamp,
            on_event,
            debug_events_logged,
            &format!(
                "SDL input: controller={} axis={axis:?} value={value}",
                controller_name
            ),
        ),
        Axis::LeftY => handle_direction_transition(
            action_states,
            unmapped_controller_sources,
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_CONTROLLER_AXIS_{axis:?}_NEG"),
            ),
            SourceId::ControllerAxis {
                instance_id,
                axis,
                direction: -1,
            },
            format!("SDL_CONTROLLER_AXIS_{axis:?}_NEG"),
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"),
            ),
            SourceId::ControllerAxis {
                instance_id,
                axis,
                direction: 1,
            },
            format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"),
            axis_direction(value),
            DEFAULT_INITIAL_REPEAT_DELAY,
            instance_id,
            &device_id,
            &controller_name,
            timestamp,
            on_event,
            debug_events_logged,
            &format!(
                "SDL input: controller={} axis={axis:?} value={value}",
                controller_name
            ),
        ),
        Axis::TriggerLeft | Axis::TriggerRight => handle_direction_transition(
            action_states,
            unmapped_controller_sources,
            None,
            SourceId::ControllerAxis {
                instance_id,
                axis,
                direction: -1,
            },
            format!("SDL_CONTROLLER_AXIS_{axis:?}_NEG"),
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"),
            ),
            SourceId::ControllerAxis {
                instance_id,
                axis,
                direction: 1,
            },
            format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"),
            trigger_axis_direction(value),
            DEFAULT_INITIAL_REPEAT_DELAY,
            instance_id,
            &device_id,
            &controller_name,
            timestamp,
            on_event,
            debug_events_logged,
            &format!(
                "SDL input: controller={} axis={axis:?} value={value}",
                controller_name
            ),
        ),
        _ => {}
    }
}

fn trigger_axis_direction(value: i16) -> i8 {
    if value >= DEAD_ZONE {
        1
    } else {
        0
    }
}

fn handle_joystick_axis_event(
    action_states: &mut HashMap<ConsoleAction, ActionState>,
    unmapped_controller_sources: &mut HashSet<SourceId>,
    instance_id: u32,
    axis_idx: u8,
    value: i16,
    timestamp: u32,
    profile_bindings: &ControllerProfileBindings,
    device_id: String,
    controller_name: String,
    on_event: &mut impl FnMut(ConsoleInputEvent),
    debug_events_logged: &mut usize,
) {
    match axis_idx {
        0 => handle_direction_transition(
            action_states,
            unmapped_controller_sources,
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"),
            ),
            SourceId::JoystickAxis {
                instance_id,
                axis_idx,
                direction: -1,
            },
            format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"),
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"),
            ),
            SourceId::JoystickAxis {
                instance_id,
                axis_idx,
                direction: 1,
            },
            format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"),
            axis_direction(value),
            DEFAULT_INITIAL_REPEAT_DELAY,
            instance_id,
            &device_id,
            &controller_name,
            timestamp,
            on_event,
            debug_events_logged,
            &format!(
                "SDL input: joystick={} axis={} value={value}",
                controller_name, axis_idx
            ),
        ),
        1 => handle_direction_transition(
            action_states,
            unmapped_controller_sources,
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"),
            ),
            SourceId::JoystickAxis {
                instance_id,
                axis_idx,
                direction: -1,
            },
            format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"),
            resolve_profile_action(
                profile_bindings,
                &device_id,
                &format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"),
            ),
            SourceId::JoystickAxis {
                instance_id,
                axis_idx,
                direction: 1,
            },
            format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"),
            axis_direction(value),
            DEFAULT_INITIAL_REPEAT_DELAY,
            instance_id,
            &device_id,
            &controller_name,
            timestamp,
            on_event,
            debug_events_logged,
            &format!(
                "SDL input: joystick={} axis={} value={value}",
                controller_name, axis_idx
            ),
        ),
        _ => {}
    }
}

fn joystick_axis_actions(
    profile_bindings: &ControllerProfileBindings,
    device_id: &str,
    axis_idx: u8,
) -> (Option<ConsoleAction>, Option<ConsoleAction>) {
    match axis_idx {
        0 => (
            resolve_profile_action(
                profile_bindings,
                device_id,
                &format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"),
            ),
            resolve_profile_action(
                profile_bindings,
                device_id,
                &format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"),
            ),
        ),
        1 => (
            resolve_profile_action(
                profile_bindings,
                device_id,
                &format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"),
            ),
            resolve_profile_action(
                profile_bindings,
                device_id,
                &format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"),
            ),
        ),
        _ => (None, None),
    }
}

fn reconcile_joystick_axis_states(
    action_states: &mut HashMap<ConsoleAction, ActionState>,
    unmapped_controller_sources: &mut HashSet<SourceId>,
    joysticks: &mut HashMap<u32, OpenJoystick>,
    profile_bindings: &ControllerProfileBindings,
    on_event: &mut impl FnMut(ConsoleInputEvent),
    debug_events_logged: &mut usize,
) {
    for (instance_id, joystick) in joysticks.iter_mut() {
        let axis_input_ready = refresh_joystick_axis_ready(*instance_id, joystick);
        let controller_name = joystick.joystick.name();
        let device_id = joystick.device_id.clone();

        for button_idx in 0..joystick.joystick.num_buttons() {
            let Ok(pressed) = joystick.joystick.button(button_idx) else {
                continue;
            };

            let button_idx = button_idx as u8;
            let source = SourceId::JoystickButton {
                instance_id: *instance_id,
                button_idx,
            };
            let token = joystick_button_token(button_idx);
            if let Some(action) = resolve_profile_action(profile_bindings, &device_id, &token) {
                if pressed {
                    if push_action(
                        action_states,
                        action,
                        source,
                        DEFAULT_INITIAL_REPEAT_DELAY,
                    ) {
                        if *debug_events_logged < 64 {
                            log_launch(&format!(
                                "SDL poll: joystick={} button={} action={} pressed=1 token={token}",
                                controller_name,
                                button_idx,
                                action.as_str()
                            ));
                            *debug_events_logged += 1;
                        }
                        emit_pressed_event(
                            on_event,
                            *instance_id,
                            &device_id,
                            &controller_name,
                            action,
                            &token,
                            0,
                        );
                    }
                } else if release_action(action_states, action, source) {
                    if *debug_events_logged < 64 {
                        log_launch(&format!(
                            "SDL poll: joystick={} button={} action={} pressed=0 token={token}",
                            controller_name,
                            button_idx,
                            action.as_str()
                        ));
                        *debug_events_logged += 1;
                    }
                    emit_released_event(
                        on_event,
                        *instance_id,
                        &device_id,
                        &controller_name,
                        action,
                        &token,
                        0,
                    );
                }
            }
        }

        for axis_idx in 0..joystick.joystick.num_axes().min(2) {
            let Ok(value) = joystick.joystick.axis(axis_idx) else {
                continue;
            };

            let axis_idx = axis_idx as u8;
            let direction = if axis_input_ready {
                axis_direction(value)
            } else {
                0
            };
            let (negative_action, positive_action) =
                joystick_axis_actions(profile_bindings, &device_id, axis_idx);

            handle_direction_transition(
                action_states,
                unmapped_controller_sources,
                negative_action,
                SourceId::JoystickAxis {
                    instance_id: *instance_id,
                    axis_idx,
                    direction: -1,
                },
                format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"),
                positive_action,
                SourceId::JoystickAxis {
                    instance_id: *instance_id,
                    axis_idx,
                    direction: 1,
                },
                format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"),
                direction,
                DEFAULT_INITIAL_REPEAT_DELAY,
                *instance_id,
                &device_id,
                &controller_name,
                0,
                on_event,
                debug_events_logged,
                &format!(
                    "SDL poll: joystick={} axis={} value={value}",
                    controller_name, axis_idx
                ),
            );
        }
    }
}

fn should_ignore_startup_axis_motion(
    joystick: &mut OpenJoystick,
    axis_idx: u8,
    _value: i16,
) -> bool {
    if joystick.axis_input_ready {
        return false;
    }

    if !joystick.pending_neutral_axes.contains(&axis_idx) {
        return false;
    }

    true
}

fn refresh_joystick_axis_ready(instance_id: u32, joystick: &mut OpenJoystick) -> bool {
    if joystick.axis_input_ready {
        return true;
    }

    let mut pending_neutral_axes = HashSet::new();
    for axis_idx in 0..joystick.joystick.num_axes().min(2) {
        if let Ok(value) = joystick.joystick.axis(axis_idx) {
            if axis_direction(value) != 0 {
                pending_neutral_axes.insert(axis_idx as u8);
            }
        }
    }

    joystick.pending_neutral_axes = pending_neutral_axes;
    if joystick.pending_neutral_axes.is_empty() {
        joystick.axis_input_ready = true;
        log_launch(&format!(
            "SDL input: joystick instance={} axis input armed after neutral sample",
            instance_id
        ));
    }

    joystick.axis_input_ready
}

fn axis_direction(value: i16) -> i8 {
    if value <= -DEAD_ZONE {
        -1
    } else if value >= DEAD_ZONE {
        1
    } else {
        0
    }
}

fn maybe_load_controller_mappings(_controller_subsystem: &sdl2::GameControllerSubsystem) {
    if let Ok(path) = env::var("OPENCONSOLE_SDL_GAMECONTROLLER_DB") {
        if !path.is_empty() {
            log_launch(&format!(
                "SDL input: controller mapping database configured at {path}"
            ));
        }
    }
}

fn open_controllers(
    controller_subsystem: &sdl2::GameControllerSubsystem,
    joystick_subsystem: &sdl2::JoystickSubsystem,
) -> HashMap<u32, GameController> {
    let mut controllers = HashMap::new();
    let Ok(count) = joystick_subsystem.num_joysticks() else {
        return controllers;
    };

    for joystick_index in 0..count {
        if !controller_subsystem.is_game_controller(joystick_index) {
            continue;
        }

        match controller_subsystem.open(joystick_index) {
            Ok(controller) => {
                let instance_id = controller.instance_id();
                log_launch(&format!(
                    "SDL input: controller opened instance={instance_id} name={} mapping={}",
                    controller.name(),
                    controller.mapping()
                ));
                controllers.insert(instance_id, controller);
            }
            Err(error) => {
                log_launch(&format!(
                    "SDL input: failed to open controller index {joystick_index}: {error}"
                ));
            }
        }
    }

    controllers
}

fn open_generic_joysticks(
    controller_subsystem: &sdl2::GameControllerSubsystem,
    joystick_subsystem: &sdl2::JoystickSubsystem,
) -> HashMap<u32, OpenJoystick> {
    let mut joysticks = HashMap::new();
    let Ok(count) = joystick_subsystem.num_joysticks() else {
        return joysticks;
    };

    for joystick_index in 0..count {
        if controller_subsystem.is_game_controller(joystick_index) {
            continue;
        }

        match joystick_subsystem.open(joystick_index) {
            Ok(joystick) => {
                let instance_id = joystick.instance_id();
                let device_id = joystick_unique_id(&joystick);
                let pending_neutral_axes = initial_pending_neutral_axes(&joystick);
                let axis_input_ready = pending_neutral_axes.is_empty();
                log_launch(&format!(
                    "SDL input: joystick opened instance={} name={} axes={} buttons={} hats={}",
                    instance_id,
                    joystick.name(),
                    joystick.num_axes(),
                    joystick.num_buttons(),
                    joystick.num_hats()
                ));
                if !pending_neutral_axes.is_empty() {
                    log_launch(&format!(
                        "SDL input: joystick instance={} waiting for neutral on startup axes {:?}",
                        instance_id, pending_neutral_axes
                    ));
                }
                joysticks.insert(
                    instance_id,
                    OpenJoystick {
                        joystick,
                        device_id,
                        pending_neutral_axes,
                        axis_input_ready,
                    },
                );
            }
            Err(error) => {
                log_launch(&format!(
                    "SDL input: failed to open joystick index {joystick_index}: {error}"
                ));
            }
        }
    }

    joysticks
}

fn controller_matches_selected(
    controller: &GameController,
    selected_device_id: Option<&str>,
) -> bool {
    match selected_device_id {
        Some(selected) => controller_unique_id(controller) == selected,
        None => true,
    }
}

fn joystick_matches_selected(joystick: &OpenJoystick, selected_device_id: Option<&str>) -> bool {
    match selected_device_id {
        Some(selected) => joystick.device_id == selected,
        None => true,
    }
}

fn initial_pending_neutral_axes(joystick: &Joystick) -> HashSet<u8> {
    let mut pending = HashSet::new();

    for axis_idx in 0..joystick.num_axes().min(2) {
        let axis_idx_u8 = axis_idx as u8;
        if let Ok(value) = joystick.axis(axis_idx) {
            if axis_direction(value) != 0 {
                pending.insert(axis_idx_u8);
            }
        }
    }

    pending
}

fn controller_unique_id(controller: &GameController) -> String {
    let name = controller.name();
    let vendor = controller
        .vendor_id()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let product = controller
        .product_id()
        .map(|value| value.to_string())
        .unwrap_or_default();
    format!(
        "kind:controller|name:{name}|vendor:{vendor}|product:{product}"
    )
}

fn joystick_unique_id(joystick: &Joystick) -> String {
    let guid = joystick.guid().to_string();
    format!(
        "kind:joystick|guid:{guid}|name:{}",
        joystick.name()
    )
}

fn list_controller_input_devices_impl() -> Vec<ControllerDeviceInfo> {
    let sdl = match sdl2::init() {
        Ok(sdl) => sdl,
        Err(error) => {
            return fallback_controller_setup_devices(&format!(
                "failed to initialize SDL: {error}"
            ));
        }
    };

    let controller_subsystem = match sdl.game_controller() {
        Ok(subsystem) => subsystem,
        Err(error) => {
            return fallback_controller_setup_devices(&format!(
                "failed to initialize SDL game controller subsystem: {error}"
            ));
        }
    };
    let joystick_subsystem = match sdl.joystick() {
        Ok(subsystem) => subsystem,
        Err(error) => {
            return fallback_controller_setup_devices(&format!(
                "failed to initialize SDL joystick subsystem: {error}"
            ));
        }
    };

    let mut devices = Vec::new();
    let count = match joystick_subsystem.num_joysticks() {
        Ok(count) => count,
        Err(error) => {
            return fallback_controller_setup_devices(&format!(
                "failed to query SDL joystick count: {error}"
            ));
        }
    };

    for joystick_index in 0..count {
        if controller_subsystem.is_game_controller(joystick_index) {
            match controller_subsystem.open(joystick_index) {
                Ok(controller) => {
                    devices.push(controller_device_info_from_controller(
                        joystick_index,
                        &controller,
                    ));
                }
                Err(error) => {
                    log_launch(&format!(
                        "SDL input: failed to enumerate controller {joystick_index}: {error}"
                    ));
                }
            }
            continue;
        }

        match joystick_subsystem.open(joystick_index) {
            Ok(joystick) => {
                devices.push(controller_device_info_from_joystick(
                    joystick_index,
                    &joystick,
                ));
            }
            Err(error) => {
                log_launch(&format!(
                    "SDL input: failed to enumerate joystick {joystick_index}: {error}"
                ));
            }
        }
    }

    if devices.is_empty() {
        return fallback_controller_setup_devices("direct SDL scan returned zero devices");
    }

    log_launch(&format!(
        "SDL device enumeration: {} device(s) exposed to controller setup",
        devices.len()
    ));

    devices
}
