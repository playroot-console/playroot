use std::env;
use std::time::{Duration, Instant};

use openconsole::input::list_all_controller_input_devices;
use sdl2::event::Event;
use sdl2::joystick::Joystick;

fn main() {
    if env::var("OPENCONSOLE_SDL_PROBE_FORCE_OFFSCREEN")
        .ok()
        .as_deref()
        == Some("1")
    {
        env::set_var("SDL_VIDEODRIVER", "offscreen");
    }

    let duration_ms = env::var("OPENCONSOLE_SDL_PROBE_DURATION_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30_000);
    let use_window = env::var("OPENCONSOLE_SDL_PROBE_USE_WINDOW").ok().as_deref() == Some("1");

    println!("OpenConsole SDL probe starting");
    println!("Duration: {} ms", duration_ms);
    println!(
        "Window mode: {}",
        if use_window { "enabled" } else { "disabled" }
    );
    println!(
        "SDL_VIDEODRIVER={}",
        env::var("SDL_VIDEODRIVER").unwrap_or_default()
    );
    println!(
        "WAYLAND_DISPLAY={}",
        env::var("WAYLAND_DISPLAY").unwrap_or_default()
    );
    println!(
        "XDG_RUNTIME_DIR={}",
        env::var("XDG_RUNTIME_DIR").unwrap_or_default()
    );
    println!("Known controller devices:");
    for device in list_all_controller_input_devices() {
        println!(
            "- {} | {} | {} | {}",
            device.event_node, device.name, device.unique_id, device.physical_path
        );
    }

    let sdl = sdl2::init().expect("failed to initialize SDL for probe");
    let joystick_subsystem = sdl
        .joystick()
        .expect("failed to initialize SDL joystick subsystem");
    let controller_subsystem = sdl
        .game_controller()
        .expect("failed to initialize SDL game controller subsystem");

    let mut _window = None;
    if use_window {
        let video = sdl
            .video()
            .expect("failed to initialize SDL video subsystem");
        let mut window = video
            .window("OpenConsole SDL Probe", 640, 360)
            .position_centered()
            .resizable()
            .build()
            .expect("failed to create SDL probe window");
        window.raise();
        window.set_grab(true);
        _window = Some(window);
    }

    let joystick_count = joystick_subsystem.num_joysticks().unwrap_or(0);
    let mut opened_joysticks: Vec<Joystick> = Vec::new();
    println!("SDL joystick count: {joystick_count}");
    for joystick_index in 0..joystick_count {
        let is_game_controller = controller_subsystem.is_game_controller(joystick_index);
        match joystick_subsystem.open(joystick_index) {
            Ok(joystick) => {
                println!(
                    "- joystick {} | instance_id={} | {} | game_controller={} | axes={} buttons={} hats={}",
                    joystick_index,
                    joystick.instance_id(),
                    joystick.name(),
                    is_game_controller,
                    joystick.num_axes(),
                    joystick.num_buttons(),
                    joystick.num_hats()
                );
                opened_joysticks.push(joystick);
            }
            Err(error) => {
                println!(
                    "- joystick {} | failed to open | game_controller={} | {}",
                    joystick_index, is_game_controller, error
                );
            }
        }
    }

    let mut event_pump = sdl.event_pump().expect("failed to create SDL event pump");

    if use_window {
        println!("SDL probe window created; press controller buttons while this window is focused");
    } else {
        println!("SDL probe running headless; press controller buttons now");
    }

    let deadline = Instant::now() + Duration::from_millis(duration_ms);
    while Instant::now() < deadline {
        for event in event_pump.poll_iter() {
            println!("SDL event: {event:?}");
            match event {
                Event::Quit { .. } => {
                    println!("SDL probe received quit event");
                    return;
                }
                Event::Window { win_event, .. } => {
                    println!("window event: {win_event:?}");
                }
                Event::JoyDeviceAdded { which, .. } => {
                    println!("joystick added: {which}");
                }
                Event::JoyDeviceRemoved { which, .. } => {
                    println!("joystick removed: {which}");
                }
                Event::JoyButtonDown {
                    which, button_idx, ..
                } => {
                    println!("joy button down: which={which} button={button_idx}");
                }
                Event::JoyButtonUp {
                    which, button_idx, ..
                } => {
                    println!("joy button up: which={which} button={button_idx}");
                }
                Event::JoyAxisMotion {
                    which,
                    axis_idx,
                    value,
                    ..
                } => {
                    println!("joy axis motion: which={which} axis={axis_idx} value={value}");
                }
                Event::JoyHatMotion {
                    which,
                    hat_idx,
                    state,
                    ..
                } => {
                    println!("joy hat motion: which={which} hat={hat_idx} state={state:?}");
                }
                Event::ControllerDeviceAdded { which, .. } => {
                    println!("controller added: {which}");
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    println!("controller removed: {which}");
                }
                Event::ControllerButtonDown { which, button, .. } => {
                    println!("controller button down: which={which} button={button:?}");
                }
                Event::ControllerButtonUp { which, button, .. } => {
                    println!("controller button up: which={which} button={button:?}");
                }
                Event::ControllerAxisMotion {
                    which, axis, value, ..
                } => {
                    println!("controller axis motion: which={which} axis={axis:?} value={value}");
                }
                _ => {}
            }
        }

        std::thread::sleep(Duration::from_millis(16));
    }

    println!("OpenConsole SDL probe finished");
}
