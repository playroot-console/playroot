use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use nix::sys::stat::{umask, Mode};
use nix::unistd::Uid;
use serde::{Deserialize, Serialize};

pub const ROOT_SOCKET_DIR: &str = "/run/openconsole";
pub const ROOT_SOCKET_PATH: &str = "/run/openconsole/supervisor.sock";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SupervisorRequest {
    Launch {
        game_index: usize,
    },
    CaptureControllerInput {
        timeout_ms: u64,
        device_id: Option<String>,
    },
    SetRuntimeMapperEnabled {
        enabled: bool,
    },
    Reboot,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SupervisorResponse {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub captured_input: Option<String>,
}

impl SupervisorResponse {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            captured_input: None,
        }
    }

    pub fn ok_with_input(message: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            captured_input: Some(input.into()),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            captured_input: None,
        }
    }
}

pub fn supervisor_socket_path() -> PathBuf {
    std::env::var("OPENCONSOLE_SUPERVISOR_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_supervisor_socket_path())
}

fn default_supervisor_socket_path() -> PathBuf {
    if Uid::effective().is_root() {
        PathBuf::from(ROOT_SOCKET_PATH)
    } else {
        std::env::temp_dir().join(format!(
            "openconsole-supervisor-{}.sock",
            Uid::effective().as_raw()
        ))
    }
}

pub fn bind_supervisor_socket(path: &Path) -> Result<UnixListener, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create socket directory {}: {error}",
                parent.display()
            )
        })?;

        if parent == Path::new(ROOT_SOCKET_DIR) {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|error| {
                format!(
                    "failed to chmod socket directory {}: {error}",
                    parent.display()
                )
            })?;
        }
    }

    if path.exists() {
        fs::remove_file(path).map_err(|error| {
            format!("failed to remove stale socket {}: {error}", path.display())
        })?;
    }

    let previous_umask = umask(Mode::from_bits_truncate(0o177));
    let listener = UnixListener::bind(path).map_err(|error| {
        format!(
            "failed to bind supervisor socket {}: {error}",
            path.display()
        )
    });
    umask(previous_umask);
    let listener = listener?;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!(
            "failed to chmod supervisor socket {}: {error}",
            path.display()
        )
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("failed to set supervisor socket nonblocking: {error}"))?;
    Ok(listener)
}

pub fn send_launch_request(
    socket_path: &Path,
    game_index: usize,
) -> Result<SupervisorResponse, String> {
    send_request(socket_path, &SupervisorRequest::Launch { game_index })
}

pub fn send_controller_capture_request(
    socket_path: &Path,
    timeout_ms: u64,
    device_id: Option<&str>,
) -> Result<SupervisorResponse, String> {
    send_request(
        socket_path,
        &SupervisorRequest::CaptureControllerInput {
            timeout_ms,
            device_id: device_id.map(|device_id| device_id.to_string()),
        },
    )
}

pub fn send_runtime_mapper_toggle_request(
    socket_path: &Path,
    enabled: bool,
) -> Result<SupervisorResponse, String> {
    send_request(
        socket_path,
        &SupervisorRequest::SetRuntimeMapperEnabled { enabled },
    )
}

pub fn send_reboot_request(socket_path: &Path) -> Result<SupervisorResponse, String> {
    send_request(socket_path, &SupervisorRequest::Reboot)
}

fn send_request(
    socket_path: &Path,
    request: &SupervisorRequest,
) -> Result<SupervisorResponse, String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| {
        format!(
            "failed to connect to supervisor socket {}: {error}",
            socket_path.display()
        )
    })?;

    let payload = serde_json::to_vec(request)
        .map_err(|error| format!("failed to serialize supervisor request: {error}"))?;
    stream
        .write_all(&payload)
        .map_err(|error| format!("failed to write supervisor request: {error}"))?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|error| format!("failed to finish supervisor request: {error}"))?;

    let mut response_buf = Vec::new();
    stream
        .read_to_end(&mut response_buf)
        .map_err(|error| format!("failed to read supervisor response: {error}"))?;

    serde_json::from_slice(&response_buf)
        .map_err(|error| format!("failed to decode supervisor response: {error}"))
}

pub fn read_request(stream: &mut UnixStream) -> Result<SupervisorRequest, String> {
    let mut request_buf = Vec::new();
    stream
        .read_to_end(&mut request_buf)
        .map_err(|error| format!("failed to read supervisor request: {error}"))?;

    serde_json::from_slice(&request_buf)
        .map_err(|error| format!("failed to decode supervisor request: {error}"))
}

pub fn write_response(
    stream: &mut UnixStream,
    response: &SupervisorResponse,
) -> Result<(), String> {
    let payload = serde_json::to_vec(response)
        .map_err(|error| format!("failed to serialize supervisor response: {error}"))?;
    stream
        .write_all(&payload)
        .map_err(|error| format!("failed to send supervisor response: {error}"))
}
