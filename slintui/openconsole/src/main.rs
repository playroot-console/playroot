use std::io::ErrorKind;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

use openconsole::audio::{refresh_audio_device_state, write_asoundrc};
use openconsole::hotkey::start_hotkey_monitor;
use openconsole::bluetooth::reconnect_saved_dualsense_devices;
use openconsole::ipc::{
    bind_supervisor_socket, read_request, supervisor_socket_path, write_response, SupervisorRequest, SupervisorResponse,
};
use openconsole::logging::log_launch;
use openconsole::controller::load_store;
use openconsole::manifest::{GameEntry, GameManifest};
use openconsole::system_settings::{apply_system_settings, load_or_create_system_settings};
use openconsole::wifi::enforce_connectivity_policy;
use openconsole::runtime_mapper::start_runtime_mapper;
use openconsole::usb_gamepad_recovery::recover_usb_gamepads_after_boot;

const UI_BINARY_NAME: &str = "openconsole-ui";
const DELAYED_USB_GAMEPAD_RECOVERY_DELAY: Duration = Duration::from_secs(10);
const AUDIO_ROUTE_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const LOOP_IDLE_SLEEP: Duration = Duration::from_millis(100);
const CURRENT_GAME_LOG_PATH: &str = "/data/godot-current.log";
const PREVIOUS_GAME_LOG_PATH: &str = "/data/godot-previous.log";
const CURRENT_UI_LOG_PATH: &str = "/data/openconsole-ui-current.log";
const PREVIOUS_UI_LOG_PATH: &str = "/data/openconsole-ui-previous.log";
const GAME_ROOT_DIR: &str = "/data/games/project";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::panic::set_hook(Box::new(|panic_info| {
        log_launch(&format!("supervisor panic: {panic_info}"));
    }));

    let socket_path = supervisor_socket_path();
    let listener = bind_supervisor_socket(&socket_path)?;

    match load_or_create_system_settings() {
        Ok(settings) => {
            if let Err(error) = apply_system_settings(&settings) {
                log_launch(&format!("failed to apply persisted system settings: {error}"));
            } else if settings.bluetooth_enabled {
                match load_store() {
                    Ok(store) => {
                        let _ = reconnect_saved_dualsense_devices(&store);
                    }
                    Err(error) => {
                        log_launch(&format!("failed to load controller store for bluetooth reconnect: {error}"));
                    }
                }
            }
        }
        Err(error) => {
            log_launch(&format!("failed to load persisted system settings: {error}"));
        }
    }

    recover_usb_gamepads_after_boot();

    match refresh_audio_device_state() {
        Ok(audio_device) => {
            log_launch(&format!("audio routing: startup selected {audio_device}"));
        }
        Err(error) => {
            log_launch(&format!("audio routing: startup refresh failed: {error}"));
        }
    }

    thread::spawn(|| {
        thread::sleep(DELAYED_USB_GAMEPAD_RECOVERY_DELAY);
        log_launch("usb gamepad recovery: running delayed post-startup pass");
        recover_usb_gamepads_after_boot();
    });

    log_launch(&format!(
        "openconsole supervisor listening on {}",
        socket_path.display()
    ));

    thread::spawn(|| loop {
        match load_or_create_system_settings() {
            Ok(settings) => {
                if let Err(error) = enforce_connectivity_policy(
                    settings.wifi_enabled,
                    settings.preferred_wifi_network.as_deref(),
                ) {
                    log_launch(&format!("wifi policy update failed: {error}"));
                }
            }
            Err(error) => {
                log_launch(&format!("wifi policy settings load failed: {error}"));
            }
        }

        thread::sleep(Duration::from_secs(3));
    });

    thread::spawn(|| {
        let mut last_audio_device = None::<String>;
        let mut last_refresh_failed = false;

        loop {
            match refresh_audio_device_state() {
                Ok(audio_device) => {
                    if last_audio_device.as_deref() != Some(audio_device.as_str()) {
                        log_launch(&format!("audio routing: selected {audio_device}"));
                        last_audio_device = Some(audio_device);
                    }
                    last_refresh_failed = false;
                }
                Err(error) => {
                    if !last_refresh_failed {
                        log_launch(&format!("audio routing: refresh failed: {error}"));
                        last_refresh_failed = true;
                    }
                }
            }

            thread::sleep(AUDIO_ROUTE_REFRESH_INTERVAL);
        }
    });

    loop {
        terminate_other_game_processes("before ui launch");
        let mut ui_runtime_mapper = start_runtime_mapper(false);
        let mut ui_child = launch_ui()?;

        match wait_for_launch_request(&listener, &mut ui_child, &mut ui_runtime_mapper) {
            Ok(Some(game)) => {
                terminate_child(&mut ui_child, "ui");
                stop_runtime_mapper(ui_runtime_mapper);
                run_game_until_exit_or_hotkey(&game);
            }
            Ok(None) => {
                stop_runtime_mapper(ui_runtime_mapper);
                log_launch("ui exited without a launch request; relaunching ui");
            }
            Err(error) => {
                log_launch(&format!("supervisor error: {error}"));
                terminate_child(&mut ui_child, "ui");
                stop_runtime_mapper(ui_runtime_mapper);
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn launch_ui() -> Result<Child, String> {
    let ui_path = ui_binary_path();
    log_launch(&format!("launching ui {}", ui_path.display()));

    let (stdout_log, stderr_log) = prepare_ui_logs()?;

    Command::new(&ui_path)
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log))
        .spawn()
        .map_err(|error| format!("failed to launch ui {}: {error}", ui_path.display()))
}

fn ui_binary_path() -> PathBuf {
    if let Ok(path) = std::env::var("OPENCONSOLE_UI_BINARY") {
        return PathBuf::from(path);
    }

    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(UI_BINARY_NAME)))
        .unwrap_or_else(|| PathBuf::from(UI_BINARY_NAME))
}

fn wait_for_launch_request(
    listener: &UnixListener,
    ui_child: &mut Child,
    ui_runtime_mapper: &mut Option<(std::sync::Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<()>)>,
) -> Result<Option<GameEntry>, String> {
    loop {
        if let Some(status) = ui_child
            .try_wait()
            .map_err(|error| format!("failed to wait for ui: {error}"))?
        {
            log_launch(&format!("ui exited with status {status}"));
            return Ok(None);
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                let request = read_request(&mut stream)?;
                match request {
                    SupervisorRequest::Launch { game_index } => {
                        match resolve_requested_game(game_index) {
                            Ok(game) => {
                                terminate_other_game_processes("before game launch");
                                write_response(&mut stream, &SupervisorResponse::ok("launch accepted"))?;
                                return Ok(Some(game));
                            }
                            Err(error) => {
                                log_launch(&format!("launch request rejected: {error}"));
                                let _ = write_response(&mut stream, &SupervisorResponse::err(error));
                            }
                        }
                    }
                    SupervisorRequest::CaptureControllerInput { .. } => {
                        let _ = write_response(
                            &mut stream,
                            &SupervisorResponse::err(
                                "controller capture is handled by the UI runtime",
                            ),
                        );
                    }
                    SupervisorRequest::SetRuntimeMapperEnabled { enabled } => {
                        if enabled {
                            if ui_runtime_mapper.is_none() {
                                *ui_runtime_mapper = start_runtime_mapper(false);
                            }
                        } else {
                            stop_runtime_mapper(ui_runtime_mapper.take());
                        }

                        let status = if enabled { "runtime mapper enabled" } else { "runtime mapper disabled" };
                        let _ = write_response(&mut stream, &SupervisorResponse::ok(status));
                    }
                    SupervisorRequest::Reboot => {
                        log_launch("supervisor received reboot request");
                        match reboot_system() {
                            Ok(()) => {
                                let response = SupervisorResponse::ok("reboot accepted");
                                let _ = write_response(&mut stream, &response);
                                std::process::exit(0);
                            }
                            Err(error) => {
                                log_launch(&format!("failed to request reboot: {error}"));
                                let _ = write_response(
                                    &mut stream,
                                    &SupervisorResponse::err(format!(
                                        "failed to request reboot: {error}"
                                    )),
                                );
                            }
                        }
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(LOOP_IDLE_SLEEP);
            }
            Err(error) => {
                return Err(format!("failed to accept supervisor request: {error}"));
            }
        }
    }
}

fn reboot_system() -> Result<(), String> {
    let status = Command::new("/sbin/reboot")
        .status()
        .map_err(|error| format!("failed to request reboot: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("reboot command exited with status {status}"))
    }
}

fn resolve_requested_game(game_index: usize) -> Result<GameEntry, String> {
    let manifest = GameManifest::load_from_default_locations()?;
    manifest
        .game_by_index(game_index)
        .cloned()
        .ok_or_else(|| format!("game index {} not found in manifest", game_index))
}

fn sanitize_game_args(args: &[String]) -> Vec<String> {
    let mut sanitized = Vec::with_capacity(args.len());
    let mut index = 0;

    while index < args.len() {
        if args[index] == "--audio-driver" {
            if let Some(value) = args.get(index + 1) {
                if value.eq_ignore_ascii_case("dummy") {
                    index += 2;
                    continue;
                }

                sanitized.push(args[index].clone());
                sanitized.push(value.clone());
                index += 2;
                continue;
            }
        }

        sanitized.push(args[index].clone());
        index += 1;
    }

    sanitized
}

fn run_game_until_exit_or_hotkey(game: &GameEntry) {
    log_launch(&format!("launching game {} ({})", game.title, game.command));

    let mut command = Command::new(&game.command);
    let cwd = if let Some(cwd) = &game.cwd {
        cwd.clone()
    } else if let Some(parent) = Path::new(&game.command).parent() {
        parent.display().to_string()
    } else {
        ".".to_string()
    };

    if let Some(cwd) = &game.cwd {
        command.current_dir(cwd);
    } else if let Some(parent) = Path::new(&game.command).parent() {
        command.current_dir(parent);
    }

    let mut selected_audio_device = None::<String>;

    match refresh_audio_device_state() {
        Ok(audio_device) => {
            let audio_home = std::env::var("HOME").unwrap_or_else(|_| "/data/home/root".to_string());
            let asoundrc_path = PathBuf::from(&audio_home).join(".asoundrc");
            if let Err(error) = write_asoundrc(&asoundrc_path, &audio_device) {
                log_launch(&format!("audio routing: failed to write {}: {error}", asoundrc_path.display()));
            } else {
                log_launch(&format!("audio routing: wrote {} for {}", asoundrc_path.display(), audio_device));
            }
            selected_audio_device = Some(audio_device.clone());
            log_launch(&format!("audio routing: launching {} with {}", game.id, audio_device));
        }
        Err(error) => {
            log_launch(&format!(
                "audio routing: failed to refresh before launching {}: {error}",
                game.id
            ));
        }
    }

    for (key, value) in &game.env {
        command.env(key, value);
    }

    for key in &game.env_remove {
        command.env_remove(key);
    }

    if let Some(audio_device) = selected_audio_device.as_deref() {
        command.env("OPENCONSOLE_AUDIO_DEVICE", audio_device);
        command.env("SDL_AUDIODRIVER", "alsa");
        command.env("AUDIODEV", "default");
    }

    let args = sanitize_game_args(&game.args);
    if args.len() != game.args.len() {
        log_launch(&format!(
            "audio routing: removed dummy audio driver override for {}",
            game.id
        ));
    }
    command.args(&args);

    log_launch(&format!("cwd={cwd}"));
    log_launch(&format!(
        "XDG_RUNTIME_DIR={}",
        display_env_override(game, "XDG_RUNTIME_DIR")
    ));
    log_launch(&format!(
        "WAYLAND_DISPLAY={}",
        display_env_override(game, "WAYLAND_DISPLAY")
    ));
    log_launch(&format!("DISPLAY={}", display_env_override(game, "DISPLAY")));
    log_launch(&format!("DRI_PRIME={}", display_env_override(game, "DRI_PRIME")));
    log_launch(&format!("SDL_AUDIODRIVER={}", command.get_envs().find_map(|(key, value)| {
        if key == "SDL_AUDIODRIVER" {
            value.map(|v| v.to_string_lossy().into_owned())
        } else {
            None
        }
    }).unwrap_or_else(|| "<unset>".to_string())));

    match prepare_game_logs() {
        Ok((stdout_log, stderr_log)) => {
            command.stdout(Stdio::from(stdout_log));
            command.stderr(Stdio::from(stderr_log));
        }
        Err(error) => {
            log_launch(&format!("failed to prepare game logs: {error}"));
            return;
        }
    }

    let Ok(mut child) = command.spawn() else {
        log_launch(&format!("failed to launch game {}", game.command));
        return;
    };

    log_launch(&format!("spawned game pid={} id={}", child.id(), game.id));

    let (hotkey_rx, hotkey_stop, hotkey_handle) = start_hotkey_monitor();
    let runtime_mapper = start_runtime_mapper(true);
    let mut hotkey_triggered = false;
    let tracked_game_exe = expected_game_executable(game);
    let mut wrapper_exited = false;
    let mut waiting_for_detached_game_logged = false;

    loop {
        if !wrapper_exited {
            match child.try_wait() {
                Ok(Some(status)) => {
                    wrapper_exited = true;
                    log_launch(&format!("game launcher for {} exited with status {status}", game.id));
                }
                Ok(None) => {}
                Err(error) => {
                    log_launch(&format!("failed while monitoring game {}: {error}", game.id));
                    break;
                }
            }
        }

        let running_game_pids = tracked_game_exe
            .as_ref()
            .map(|exe| running_pids_for_executable(exe))
            .unwrap_or_default();

        if wrapper_exited {
            if running_game_pids.is_empty() {
                log_launch(&format!("game {} exited", game.id));
                break;
            }

            if !waiting_for_detached_game_logged {
                log_launch(&format!(
                    "game {} launcher exited but game process is still running; waiting for {} process(es)",
                    game.id,
                    running_game_pids.len()
                ));
                waiting_for_detached_game_logged = true;
            }
        }

        if hotkey_rx.try_recv().is_ok() {
            hotkey_triggered = true;
            log_launch(&format!("failsafe hotkey requested termination for game {}", game.id));
            if !wrapper_exited {
                if let Err(error) = child.kill() {
                    log_launch(&format!("failed to kill game launcher {}: {error}", game.id));
                }
            }

            for pid in running_game_pids {
                if let Err(error) = kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
                    log_launch(&format!("failed to signal game pid {}: {error}", pid));
                }
            }
        }

        thread::sleep(LOOP_IDLE_SLEEP);
    }

    hotkey_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = hotkey_handle.join();

    if let Some((mapper_stop, mapper_handle)) = runtime_mapper {
        mapper_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = mapper_handle.join();
    }

    if hotkey_triggered {
        let _ = child.wait();
        log_launch(&format!("game {} terminated by failsafe hotkey", game.id));
    }
}

fn prepare_game_logs() -> Result<(std::fs::File, std::fs::File), String> {
    let _ = std::fs::rename(CURRENT_GAME_LOG_PATH, PREVIOUS_GAME_LOG_PATH);

    let stdout_log = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(CURRENT_GAME_LOG_PATH)
        .map_err(|error| format!("failed to open {CURRENT_GAME_LOG_PATH}: {error}"))?;

    let stderr_log = stdout_log
        .try_clone()
        .map_err(|error| format!("failed to clone {CURRENT_GAME_LOG_PATH} handle: {error}"))?;

    Ok((stdout_log, stderr_log))
}

fn prepare_ui_logs() -> Result<(std::fs::File, std::fs::File), String> {
    let _ = std::fs::rename(CURRENT_UI_LOG_PATH, PREVIOUS_UI_LOG_PATH);

    let stdout_log = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(CURRENT_UI_LOG_PATH)
        .map_err(|error| format!("failed to open {CURRENT_UI_LOG_PATH}: {error}"))?;

    let stderr_log = stdout_log
        .try_clone()
        .map_err(|error| format!("failed to clone {CURRENT_UI_LOG_PATH} handle: {error}"))?;

    Ok((stdout_log, stderr_log))
}

fn display_env_override(game: &GameEntry, key: &str) -> String {
    if game.env_remove.iter().any(|entry| entry == key) {
        return "<unset>".to_string();
    }

    if let Some(value) = game.env.get(key) {
        return value.clone();
    }

    std::env::var(key).unwrap_or_else(|_| "<unset>".to_string())
}

fn terminate_child(child: &mut Child, label: &str) {
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            log_launch(&format!("terminated lingering {label} process"));
        }
        Err(error) => {
            log_launch(&format!("failed to inspect {label} process: {error}"));
        }
    }
}

fn expected_game_executable(game: &GameEntry) -> Option<PathBuf> {
    let game_bin = game.env.get("OPENCONSOLE_GAME_BIN")?;
    if game_bin.is_empty() || game_bin.contains('/') {
        return None;
    }

    Some(PathBuf::from("/data/games/project").join(game_bin))
}

fn running_pids_for_executable(expected_exe: &Path) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !file_name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }

        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };

        let exe_path = entry.path().join("exe");
        let Ok(link_target) = fs::read_link(exe_path) else {
            continue;
        };

        if link_target == expected_exe {
            pids.push(pid);
        }
    }

    pids
}

fn terminate_other_game_processes(reason: &str) {
    let pids = running_game_pids();
    if pids.is_empty() {
        return;
    }

    log_launch(&format!(
        "terminating {} lingering game process(es) {}",
        pids.len(),
        reason
    ));

    for pid in pids {
        if let Err(error) = kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
            log_launch(&format!("failed to terminate lingering game pid {}: {error}", pid));
        }
    }
}

fn running_game_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !file_name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }

        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };

        let exe_path = entry.path().join("exe");
        let Ok(link_target) = fs::read_link(exe_path) else {
            continue;
        };

        if link_target.starts_with(GAME_ROOT_DIR)
            && link_target
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("arm64"))
                .unwrap_or(false)
        {
            pids.push(pid);
        }
    }

    pids
}

fn stop_runtime_mapper(mapper: Option<(std::sync::Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<()>)>) {
    if let Some((mapper_stop, mapper_handle)) = mapper {
        mapper_stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = mapper_handle.join();
    }
}