use std::collections::HashMap;
#[cfg(feature = "sdl-input")]
use std::collections::HashSet;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use evdev::uinput::VirtualDeviceBuilder;
use evdev::{AttributeSet, EventType, InputEvent, Key};
#[cfg(feature = "sdl-input")]
use sdl2::controller::{Axis, Button, GameController};
#[cfg(feature = "sdl-input")]
use sdl2::event::Event;
#[cfg(feature = "sdl-input")]
use sdl2::joystick::Joystick;

use crate::controller::{controller_config_path, load_store};
use crate::logging::log_launch;

const POLL_INTERVAL: Duration = Duration::from_millis(8);
const RELOAD_INTERVAL_TICKS: u32 = 125;
#[cfg(feature = "sdl-input")]
const REOPEN_INTERVAL_TICKS: u32 = 125;
#[cfg(feature = "sdl-input")]
const DEAD_ZONE: i16 = 8_000;

pub fn start_runtime_mapper(
    grab_devices: bool,
) -> Option<(Arc<AtomicBool>, thread::JoinHandle<()>)> {
    if !grab_devices {
        log_launch(
            "runtime mapper disabled for UI runtime: UI handles controllers via SDL and keyboards via raw input",
        );
        return None;
    }

    let input_to_output = match load_input_to_output_map() {
        Ok(map) => map,
        Err(error) => {
            log_launch(&format!(
                "runtime mapper: initial mapping load failed, continuing with empty map: {error}"
            ));
            HashMap::new()
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || run_mapper_backend(stop_flag, input_to_output));
    Some((stop, handle))
}

pub fn stop_runtime_mapper(handle: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)>) {
    if let Some((stop, join_handle)) = handle {
        stop.store(true, Ordering::Relaxed);
        let _ = join_handle.join();
    }
}

#[cfg(feature = "sdl-input")]
struct OpenJoystick {
    joystick: Joystick,
    device_id: String,
    pending_neutral_axes: HashSet<u8>,
    axis_input_ready: bool,
}

#[cfg(feature = "sdl-input")]
fn run_mapper_backend(stop: Arc<AtomicBool>, mut input_to_output: HashMap<String, Vec<Key>>) {
    let sdl = match sdl2::init() {
        Ok(sdl) => sdl,
        Err(error) => {
            log_launch(&format!("runtime mapper: failed to initialize SDL: {error}"));
            return;
        }
    };

    let controller_subsystem = match sdl.game_controller() {
        Ok(subsystem) => subsystem,
        Err(error) => {
            log_launch(&format!(
                "runtime mapper: failed to initialize SDL game controller subsystem: {error}"
            ));
            return;
        }
    };

    let joystick_subsystem = match sdl.joystick() {
        Ok(subsystem) => subsystem,
        Err(error) => {
            log_launch(&format!(
                "runtime mapper: failed to initialize SDL joystick subsystem: {error}"
            ));
            return;
        }
    };

    let mut event_pump = match sdl.event_pump() {
        Ok(event_pump) => event_pump,
        Err(error) => {
            log_launch(&format!("runtime mapper: failed to create SDL event pump: {error}"));
            return;
        }
    };

    let mut virtual_keyboard: Option<evdev::uinput::VirtualDevice> = None;
    let mut virtual_keyboard_error_logged = false;
    let config_path = controller_config_path();
    let mut last_config_mtime = config_mtime(&config_path);
    let mut allowed_device_ids = configured_controller_ids().unwrap_or_default();
    let mut allowed_device_aliases = expand_device_id_aliases(&allowed_device_ids);
    let mut token_states: HashMap<String, i32> = HashMap::new();
    let mut unmatched_logged = 0usize;
    let mut reload_tick = 0u32;
    let mut reopen_tick = 0u32;

    let mut controllers = HashMap::new();
    let mut joysticks = HashMap::new();
    refresh_controllers(
        &controller_subsystem,
        &joystick_subsystem,
        &mut controllers,
        &mut joysticks,
    );
    let mut had_any_devices = !(controllers.is_empty() && joysticks.is_empty());

    if !had_any_devices {
        log_launch("runtime mapper: no SDL controller devices found yet; waiting for device");
    }

    if input_to_output.is_empty() {
        log_launch("runtime mapper: no controller mappings loaded yet; waiting for config");
    }

    log_launch(&format!(
        "runtime mapper: loaded {} mapped input tokens",
        input_to_output.len()
    ));
    for device_id in allowed_device_ids.iter().take(8) {
        log_launch(&format!("runtime mapper: configured device {device_id}"));
    }
    for token in input_to_output.keys().take(16) {
        log_launch(&format!("runtime mapper: mapped token {token}"));
    }
    log_launch("runtime mapper: started");

    while !stop.load(Ordering::Relaxed) {
        if virtual_keyboard.is_none() {
            match build_virtual_keyboard() {
                Ok(device) => {
                    virtual_keyboard = Some(device);
                    virtual_keyboard_error_logged = false;
                    log_launch("runtime mapper: virtual keyboard ready");
                }
                Err(error) => {
                    if !virtual_keyboard_error_logged {
                        log_launch(&format!(
                            "runtime mapper: virtual keyboard not ready yet: {error}"
                        ));
                        virtual_keyboard_error_logged = true;
                    }
                    thread::sleep(POLL_INTERVAL);
                    continue;
                }
            }
        }

        let Some(virtual_keyboard) = virtual_keyboard.as_mut() else {
            thread::sleep(POLL_INTERVAL);
            continue;
        };

        if reload_tick == 0 {
            let current_mtime = config_mtime(&config_path);
            if current_mtime != last_config_mtime || input_to_output.is_empty() {
                last_config_mtime = current_mtime;
                match load_input_to_output_map() {
                    Ok(reloaded) => {
                        input_to_output = reloaded;
                        unmatched_logged = 0;
                        log_launch(&format!(
                            "runtime mapper: reloaded {} mapped input tokens",
                            input_to_output.len()
                        ));
                    }
                    Err(error) => {
                        log_launch(&format!("runtime mapper: failed to reload mappings: {error}"));
                    }
                }

                allowed_device_ids = configured_controller_ids().unwrap_or_default();
                allowed_device_aliases = expand_device_id_aliases(&allowed_device_ids);
                release_all_active_tokens(virtual_keyboard, &input_to_output, &mut token_states);
            }
        }

        if reopen_tick == 0 && !had_any_devices {
            refresh_controllers(
                &controller_subsystem,
                &joystick_subsystem,
                &mut controllers,
                &mut joysticks,
            );

            let have_devices_now = !(controllers.is_empty() && joysticks.is_empty());
            if have_devices_now && !had_any_devices {
                log_launch("runtime mapper: controller devices detected");
            }
            had_any_devices = have_devices_now;
        }

        for joystick in joysticks.values_mut() {
            let _ = refresh_joystick_axis_ready(joystick);
        }

        for event in event_pump.poll_iter() {
            match event {
                Event::ControllerButtonDown { which, button, .. } => {
                    let Some(controller) = controllers.get(&which) else {
                        continue;
                    };
                    let device_id = controller_unique_id(controller);
                    if !device_id_allowed(&device_id, &allowed_device_aliases) {
                        continue;
                    }

                    let token = controller_button_token(button);
                    emit_mapped_token(
                        virtual_keyboard,
                        &input_to_output,
                        &token,
                        1,
                        &mut token_states,
                        &mut unmatched_logged,
                        "sdl controller",
                    );
                }
                Event::ControllerButtonUp { which, button, .. } => {
                    let Some(controller) = controllers.get(&which) else {
                        continue;
                    };
                    let device_id = controller_unique_id(controller);
                    if !device_id_allowed(&device_id, &allowed_device_aliases) {
                        continue;
                    }

                    let token = controller_button_token(button);
                    emit_mapped_token(
                        virtual_keyboard,
                        &input_to_output,
                        &token,
                        0,
                        &mut token_states,
                        &mut unmatched_logged,
                        "sdl controller",
                    );
                }
                Event::ControllerAxisMotion {
                    which, axis, value, ..
                } => {
                    let Some(controller) = controllers.get(&which) else {
                        continue;
                    };
                    let device_id = controller_unique_id(controller);
                    if !device_id_allowed(&device_id, &allowed_device_aliases) {
                        continue;
                    }

                    handle_direction_tokens(
                        virtual_keyboard,
                        &input_to_output,
                        &format!("SDL_CONTROLLER_AXIS_{axis:?}_NEG"),
                        &format!("SDL_CONTROLLER_AXIS_{axis:?}_POS"),
                        controller_axis_direction(axis, value),
                        &mut token_states,
                        &mut unmatched_logged,
                        "sdl controller",
                    );
                }
                Event::JoyButtonDown {
                    which, button_idx, ..
                } => {
                    let Some(joystick) = joysticks.get(&which) else {
                        continue;
                    };
                    if !device_id_allowed(&joystick.device_id, &allowed_device_aliases) {
                        continue;
                    }

                    let token = joystick_button_token(button_idx);
                    emit_mapped_token(
                        virtual_keyboard,
                        &input_to_output,
                        &token,
                        1,
                        &mut token_states,
                        &mut unmatched_logged,
                        "sdl joystick",
                    );
                }
                Event::JoyButtonUp {
                    which, button_idx, ..
                } => {
                    let Some(joystick) = joysticks.get(&which) else {
                        continue;
                    };
                    if !device_id_allowed(&joystick.device_id, &allowed_device_aliases) {
                        continue;
                    }

                    let token = joystick_button_token(button_idx);
                    emit_mapped_token(
                        virtual_keyboard,
                        &input_to_output,
                        &token,
                        0,
                        &mut token_states,
                        &mut unmatched_logged,
                        "sdl joystick",
                    );
                }
                Event::JoyAxisMotion {
                    which,
                    axis_idx,
                    value,
                    ..
                } => {
                    let Some(joystick) = joysticks.get_mut(&which) else {
                        continue;
                    };
                    if !device_id_allowed(&joystick.device_id, &allowed_device_aliases) {
                        continue;
                    }
                    if !refresh_joystick_axis_ready(joystick) {
                        continue;
                    }

                    handle_direction_tokens(
                        virtual_keyboard,
                        &input_to_output,
                        &format!("SDL_JOYSTICK_AXIS_{axis_idx}_NEG"),
                        &format!("SDL_JOYSTICK_AXIS_{axis_idx}_POS"),
                        axis_direction(value),
                        &mut token_states,
                        &mut unmatched_logged,
                        "sdl joystick",
                    );
                }
                Event::ControllerDeviceAdded { which, .. } => {
                    if let Some((instance_id, controller)) = open_controller_by_index(&controller_subsystem, which)
                    {
                        if controllers.insert(instance_id, controller).is_none() {
                            log_launch(&format!(
                                "runtime mapper: SDL controller connected instance={instance_id}"
                            ));
                        }
                    }
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    if controllers.remove(&which).is_some() {
                        release_all_active_tokens(virtual_keyboard, &input_to_output, &mut token_states);
                        log_launch(&format!(
                            "runtime mapper: SDL controller removed instance={which}"
                        ));
                    }
                }
                Event::JoyDeviceAdded { which, .. } => {
                    if controller_subsystem.is_game_controller(which) {
                        continue;
                    }
                    if let Some((instance_id, joystick)) = open_joystick_by_index(&joystick_subsystem, which) {
                        if joysticks.insert(instance_id, joystick).is_none() {
                            log_launch(&format!(
                                "runtime mapper: SDL joystick connected instance={instance_id}"
                            ));
                        }
                    }
                }
                Event::JoyDeviceRemoved { which, .. } => {
                    if joysticks.remove(&which).is_some() {
                        release_all_active_tokens(virtual_keyboard, &input_to_output, &mut token_states);
                        log_launch(&format!(
                            "runtime mapper: SDL joystick removed instance={which}"
                        ));
                    }
                }
                _ => {}
            }
        }

        reload_tick = (reload_tick + 1) % RELOAD_INTERVAL_TICKS;
        reopen_tick = (reopen_tick + 1) % REOPEN_INTERVAL_TICKS;
        thread::sleep(POLL_INTERVAL);
    }

    if let Some(virtual_keyboard) = virtual_keyboard.as_mut() {
        release_all_active_tokens(virtual_keyboard, &input_to_output, &mut token_states);
    }

    log_launch("runtime mapper: stopped");
}

#[cfg(not(feature = "sdl-input"))]
fn run_mapper_backend(stop: Arc<AtomicBool>, _input_to_output: HashMap<String, Vec<Key>>) {
    log_launch("runtime mapper disabled: binary was built without sdl-input");
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(POLL_INTERVAL);
    }
    log_launch("runtime mapper: stopped");
}

#[cfg(feature = "sdl-input")]
fn emit_mapped_token(
    virtual_keyboard: &mut evdev::uinput::VirtualDevice,
    input_to_output: &HashMap<String, Vec<Key>>,
    input_token: &str,
    value: i32,
    token_states: &mut HashMap<String, i32>,
    unmatched_logged: &mut usize,
    backend: &str,
) {
    let previous = token_states.get(input_token).copied().unwrap_or(-1);
    if previous == value {
        return;
    }
    token_states.insert(input_token.to_string(), value);

    if let Some(output_keys) = mapped_keys_for_token(input_to_output, input_token) {
        if value == 1 {
            log_launch(&format!(
                "runtime mapper: recognized {backend} token {input_token} -> {}",
                output_key_names(output_keys)
            ));
        }

        let mapped_events: Vec<InputEvent> = output_keys
            .iter()
            .map(|mapped_key| InputEvent::new(EventType::KEY, mapped_key.code(), value))
            .collect();

        if let Err(error) = virtual_keyboard.emit(&mapped_events) {
            log_launch(&format!(
                "runtime mapper: failed to emit mapped key for {}: {error}",
                input_token
            ));
        }
    } else if value == 1 && *unmatched_logged < 32 {
        log_launch(&format!(
            "runtime mapper: unmatched {backend} token {input_token}"
        ));
        *unmatched_logged += 1;
    }
}

#[cfg(feature = "sdl-input")]
fn handle_direction_tokens(
    virtual_keyboard: &mut evdev::uinput::VirtualDevice,
    input_to_output: &HashMap<String, Vec<Key>>,
    negative_token: &str,
    positive_token: &str,
    direction: i8,
    token_states: &mut HashMap<String, i32>,
    unmatched_logged: &mut usize,
    backend: &str,
) {
    match direction {
        -1 => {
            emit_mapped_token(
                virtual_keyboard,
                input_to_output,
                negative_token,
                1,
                token_states,
                unmatched_logged,
                backend,
            );
            emit_mapped_token(
                virtual_keyboard,
                input_to_output,
                positive_token,
                0,
                token_states,
                unmatched_logged,
                backend,
            );
        }
        1 => {
            emit_mapped_token(
                virtual_keyboard,
                input_to_output,
                negative_token,
                0,
                token_states,
                unmatched_logged,
                backend,
            );
            emit_mapped_token(
                virtual_keyboard,
                input_to_output,
                positive_token,
                1,
                token_states,
                unmatched_logged,
                backend,
            );
        }
        _ => {
            emit_mapped_token(
                virtual_keyboard,
                input_to_output,
                negative_token,
                0,
                token_states,
                unmatched_logged,
                backend,
            );
            emit_mapped_token(
                virtual_keyboard,
                input_to_output,
                positive_token,
                0,
                token_states,
                unmatched_logged,
                backend,
            );
        }
    }
}

#[cfg(feature = "sdl-input")]
fn release_all_active_tokens(
    virtual_keyboard: &mut evdev::uinput::VirtualDevice,
    input_to_output: &HashMap<String, Vec<Key>>,
    token_states: &mut HashMap<String, i32>,
) {
    let active_tokens = token_states
        .iter()
        .filter_map(|(token, value)| if *value == 1 { Some(token.clone()) } else { None })
        .collect::<Vec<_>>();

    for token in active_tokens {
        if let Some(output_keys) = mapped_keys_for_token(input_to_output, &token) {
            let mapped_events: Vec<InputEvent> = output_keys
                .iter()
                .map(|mapped_key| InputEvent::new(EventType::KEY, mapped_key.code(), 0))
                .collect();
            let _ = virtual_keyboard.emit(&mapped_events);
        }
        token_states.insert(token, 0);
    }
}

#[cfg(feature = "sdl-input")]
fn controller_axis_direction(axis: Axis, value: i16) -> i8 {
    match axis {
        Axis::LeftX | Axis::LeftY => axis_direction(value),
        Axis::TriggerLeft | Axis::TriggerRight => trigger_axis_direction(value),
        _ => 0,
    }
}

#[cfg(feature = "sdl-input")]
fn axis_direction(value: i16) -> i8 {
    if value <= -DEAD_ZONE {
        -1
    } else if value >= DEAD_ZONE {
        1
    } else {
        0
    }
}

#[cfg(feature = "sdl-input")]
fn trigger_axis_direction(value: i16) -> i8 {
    if value >= DEAD_ZONE {
        1
    } else {
        0
    }
}

#[cfg(feature = "sdl-input")]
fn controller_button_token(button: Button) -> String {
    match button {
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
    }
}

#[cfg(feature = "sdl-input")]
fn joystick_button_token(button_idx: u8) -> String {
    format!("SDL_JOYSTICK_BUTTON_{button_idx}")
}

#[cfg(feature = "sdl-input")]
fn refresh_controllers(
    controller_subsystem: &sdl2::GameControllerSubsystem,
    joystick_subsystem: &sdl2::JoystickSubsystem,
    controllers: &mut HashMap<u32, GameController>,
    joysticks: &mut HashMap<u32, OpenJoystick>,
) {
    let Ok(count) = joystick_subsystem.num_joysticks() else {
        return;
    };

    for joystick_index in 0..count {
        if controller_subsystem.is_game_controller(joystick_index) {
            if let Some((instance_id, controller)) = open_controller_by_index(controller_subsystem, joystick_index) {
                controllers.entry(instance_id).or_insert(controller);
            }
        } else if let Some((instance_id, joystick)) = open_joystick_by_index(joystick_subsystem, joystick_index) {
            joysticks.entry(instance_id).or_insert(joystick);
        }
    }
}

#[cfg(feature = "sdl-input")]
fn open_controller_by_index(
    controller_subsystem: &sdl2::GameControllerSubsystem,
    joystick_index: u32,
) -> Option<(u32, GameController)> {
    match controller_subsystem.open(joystick_index) {
        Ok(controller) => {
            let instance_id = controller.instance_id();
            log_launch(&format!(
                "runtime mapper: SDL controller opened instance={instance_id} name={}",
                controller.name()
            ));
            Some((instance_id, controller))
        }
        Err(error) => {
            log_launch(&format!(
                "runtime mapper: failed to open SDL controller index {joystick_index}: {error}"
            ));
            None
        }
    }
}

#[cfg(feature = "sdl-input")]
fn open_joystick_by_index(
    joystick_subsystem: &sdl2::JoystickSubsystem,
    joystick_index: u32,
) -> Option<(u32, OpenJoystick)> {
    match joystick_subsystem.open(joystick_index) {
        Ok(joystick) => {
            let instance_id = joystick.instance_id();
            let device_id = joystick_unique_id(&joystick);
            let pending_neutral_axes = initial_pending_neutral_axes(&joystick);
            let axis_input_ready = pending_neutral_axes.is_empty();
            log_launch(&format!(
                "runtime mapper: SDL joystick opened instance={} name={} axes={} buttons={}",
                instance_id,
                joystick.name(),
                joystick.num_axes(),
                joystick.num_buttons(),
            ));
            Some((
                instance_id,
                OpenJoystick {
                    joystick,
                    device_id,
                    pending_neutral_axes,
                    axis_input_ready,
                },
            ))
        }
        Err(error) => {
            log_launch(&format!(
                "runtime mapper: failed to open SDL joystick index {joystick_index}: {error}"
            ));
            None
        }
    }
}

#[cfg(feature = "sdl-input")]
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

#[cfg(feature = "sdl-input")]
fn refresh_joystick_axis_ready(joystick: &mut OpenJoystick) -> bool {
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
            "runtime mapper: SDL joystick instance={} axis input armed after neutral sample",
            joystick.joystick.instance_id()
        ));
    }

    joystick.axis_input_ready
}

#[cfg(feature = "sdl-input")]
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

#[cfg(feature = "sdl-input")]
fn joystick_unique_id(joystick: &Joystick) -> String {
    let guid = joystick.guid().to_string();
    format!("kind:joystick|guid:{guid}|name:{}", joystick.name())
}

#[cfg(feature = "sdl-input")]
fn device_id_allowed(device_id: &str, allowed_aliases: &HashSet<String>) -> bool {
    if allowed_aliases.is_empty() {
        return false;
    }

    device_id_aliases(device_id)
        .into_iter()
        .any(|alias| allowed_aliases.contains(&alias))
}

#[cfg(feature = "sdl-input")]
fn expand_device_id_aliases(device_ids: &[String]) -> HashSet<String> {
    let mut aliases = HashSet::new();
    for device_id in device_ids {
        aliases.extend(device_id_aliases(device_id));
    }
    aliases
}

#[cfg(feature = "sdl-input")]
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

#[cfg(feature = "sdl-input")]
fn segment_value(device_id: &str, prefix: &str) -> Option<String> {
    device_id
        .split('|')
        .find_map(|segment| segment.strip_prefix(prefix).map(str::to_string))
}

fn configured_controller_ids() -> Result<Vec<String>, String> {
    let store = load_store()?;
    Ok(store
        .controllers
        .into_iter()
        .filter_map(|profile| profile.device_id)
        .collect())
}

fn load_input_to_output_map() -> Result<HashMap<String, Vec<Key>>, String> {
    let store = load_store()?;
    let mut map: HashMap<String, Vec<Key>> = HashMap::new();

    for profile in &store.controllers {
        for (action, input_token) in &profile.mappings {
            let output_keys = output_keys_for_action(action);
            if output_keys.is_empty() {
                continue;
            }

            merge_mapping(&mut map, input_token, &output_keys);

            let canonical = canonical_input_token(input_token);
            if canonical != *input_token {
                merge_mapping(&mut map, &canonical, &output_keys);
            }

            for alias in action_token_aliases(action) {
                merge_mapping(&mut map, &alias, &output_keys);
            }
        }
    }

    Ok(map)
}

fn mapped_keys_for_token<'a>(input_to_output: &'a HashMap<String, Vec<Key>>, input_token: &str) -> Option<&'a Vec<Key>> {
    if let Some(keys) = input_to_output.get(input_token) {
        return Some(keys);
    }

    let canonical = canonical_input_token(input_token);
    if canonical == input_token {
        return None;
    }

    input_to_output.get(&canonical)
}

fn merge_mapping(map: &mut HashMap<String, Vec<Key>>, input_token: &str, output_keys: &[Key]) {
    map.entry(input_token.to_string())
        .and_modify(|existing| {
            for key in output_keys {
                if !existing.contains(key) {
                    existing.push(*key);
                }
            }
        })
        .or_insert_with(|| output_keys.to_vec());
}

fn canonical_input_token(input_token: &str) -> String {
    if let Some(button) = input_token.strip_prefix("SDL_JOYSTICK_BUTTON_") {
        return format!("JS_BTN_{button}");
    }

    if let Some(axis) = input_token.strip_prefix("SDL_JOYSTICK_AXIS_") {
        return format!("JS_AXIS_{axis}");
    }

    if input_token.starts_with("SDL_CONTROLLER_BUTTON_") || input_token.starts_with("SDL_CONTROLLER_AXIS_") {
        return input_token.to_string();
    }

    if let Some(rest) = input_token.strip_prefix("hidraw") {
        let digit_count = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
        if digit_count > 0 {
            let suffix = &rest[digit_count..];
            if suffix.starts_with("_B") {
                return format!("HIDRAW{suffix}");
            }
        }
    }

    input_token.to_string()
}

fn config_mtime(path: &std::path::Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|metadata| metadata.modified()).ok()
}

fn action_token_aliases(action: &str) -> Vec<String> {
    if action.starts_with("Move Left") {
        return vec![
            "LeftArrow".to_string(),
            "JS_AXIS_0_NEG".to_string(),
            "SDL_JOYSTICK_AXIS_0_NEG".to_string(),
            "SDL_CONTROLLER_BUTTON_DPAD_LEFT".to_string(),
            "SDL_CONTROLLER_AXIS_LeftX_NEG".to_string(),
        ];
    }
    if action.starts_with("Move Right") {
        return vec![
            "RightArrow".to_string(),
            "JS_AXIS_0_POS".to_string(),
            "SDL_JOYSTICK_AXIS_0_POS".to_string(),
            "SDL_CONTROLLER_BUTTON_DPAD_RIGHT".to_string(),
            "SDL_CONTROLLER_AXIS_LeftX_POS".to_string(),
        ];
    }
    if action.starts_with("Move Up") {
        return vec![
            "UpArrow".to_string(),
            "JS_AXIS_1_NEG".to_string(),
            "SDL_JOYSTICK_AXIS_1_NEG".to_string(),
            "SDL_CONTROLLER_BUTTON_DPAD_UP".to_string(),
            "SDL_CONTROLLER_AXIS_LeftY_NEG".to_string(),
        ];
    }
    if action.starts_with("Move Down") {
        return vec![
            "DownArrow".to_string(),
            "JS_AXIS_1_POS".to_string(),
            "SDL_JOYSTICK_AXIS_1_POS".to_string(),
            "SDL_CONTROLLER_BUTTON_DPAD_DOWN".to_string(),
            "SDL_CONTROLLER_AXIS_LeftY_POS".to_string(),
        ];
    }
    if action.starts_with("Button A") {
        return vec![
            "Space".to_string(),
            "SDL_JOYSTICK_BUTTON_1".to_string(),
            "SDL_CONTROLLER_BUTTON_A".to_string(),
            "SDL_CONTROLLER_BUTTON_X".to_string(),
        ];
    }
    if action.starts_with("Button B") {
        return vec![
            "Return".to_string(),
            "SDL_JOYSTICK_BUTTON_2".to_string(),
            "SDL_CONTROLLER_BUTTON_B".to_string(),
            "SDL_CONTROLLER_BUTTON_Y".to_string(),
        ];
    }
    if action.starts_with("Start") {
        return vec![
            "Escape".to_string(),
            "BTN_BASE4".to_string(),
            "SDL_JOYSTICK_BUTTON_9".to_string(),
            "SDL_CONTROLLER_BUTTON_START".to_string(),
        ];
    }
    if action.starts_with("Select") {
        return vec![
            "Return".to_string(),
            "JS_BTN_8".to_string(),
            "SDL_JOYSTICK_BUTTON_8".to_string(),
            "SDL_CONTROLLER_BUTTON_BACK".to_string(),
        ];
    }

    Vec::new()
}

fn output_keys_for_action(action: &str) -> Vec<Key> {
    if action.starts_with("Move Up") {
        return vec![Key::KEY_UP, Key::KEY_W];
    }
    if action.starts_with("Move Down") {
        return vec![Key::KEY_DOWN, Key::KEY_S];
    }
    if action.starts_with("Move Left") {
        return vec![Key::KEY_LEFT, Key::KEY_A];
    }
    if action.starts_with("Move Right") {
        return vec![Key::KEY_RIGHT, Key::KEY_D];
    }
    if action.starts_with("Button A") {
        return vec![Key::KEY_SPACE];
    }
    if action.starts_with("Button B") {
        return vec![Key::KEY_ENTER];
    }
    if action.starts_with("Start") {
        return vec![Key::KEY_ESC];
    }
    if action.starts_with("Select") {
        return vec![Key::KEY_ENTER];
    }

    Vec::new()
}

fn output_key_names(output_keys: &[Key]) -> String {
    output_keys
        .iter()
        .map(|key| format!("{key:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn build_virtual_keyboard() -> Result<evdev::uinput::VirtualDevice, String> {
    let mut keys = AttributeSet::<Key>::new();
    for key in [
        Key::KEY_UP,
        Key::KEY_DOWN,
        Key::KEY_LEFT,
        Key::KEY_RIGHT,
        Key::KEY_W,
        Key::KEY_A,
        Key::KEY_S,
        Key::KEY_D,
        Key::KEY_SPACE,
        Key::KEY_ENTER,
        Key::KEY_ESC,
    ] {
        keys.insert(key);
    }

    VirtualDeviceBuilder::new()
        .map_err(|error| format!("uinput init failed: {error}"))?
        .name("OpenConsole Runtime Mapper")
        .with_keys(&keys)
        .map_err(|error| format!("uinput key registration failed: {error}"))?
        .build()
        .map_err(|error| format!("uinput build failed: {error}"))
}