use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

pub const DEFAULT_LAUNCH_LOG_PATH: &str = "/data/openconsole-launch.log";

pub fn launch_log_path() -> PathBuf {
    std::env::var("OPENCONSOLE_LAUNCH_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_LAUNCH_LOG_PATH))
}

pub fn log_launch(message: &str) {
    eprintln!("{message}");

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(launch_log_path())
    {
        let _ = writeln!(file, "{message}");
    }
}
