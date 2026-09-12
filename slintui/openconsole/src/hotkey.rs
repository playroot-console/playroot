use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::keyboard_input::{KeyboardBackend, RawKeyboardEvent, KEY_LEFTCTRL, KEY_RIGHTCTRL};
use crate::logging::log_launch;

const HOLD_DURATION: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub fn start_hotkey_monitor() -> (Receiver<()>, Arc<AtomicBool>, thread::JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let handle = thread::spawn(move || watch_hotkey(stop_flag, sender));

    (receiver, stop, handle)
}

fn watch_hotkey(stop: Arc<AtomicBool>, sender: Sender<()>) {
    let mut keyboard_backend = KeyboardBackend::new();

    log_launch("failsafe hotkey: watching for LeftCtrl+RightCtrl held for 2 seconds via raw keyboard input");

    let mut left_ctrl_pressed = false;
    let mut right_ctrl_pressed = false;
    let mut hold_started_at: Option<Instant> = None;
    let mut trigger_sent = false;

    while !stop.load(Ordering::Relaxed) {
        for event in keyboard_backend.poll_events() {
            match event {
                RawKeyboardEvent::Key {
                    key_code, value, ..
                } => {
                    let is_pressed = value == 1;
                    let is_released = value == 0;
                    if key_code == KEY_LEFTCTRL {
                        left_ctrl_pressed = is_pressed || (!is_released && left_ctrl_pressed);
                    } else if key_code == KEY_RIGHTCTRL {
                        right_ctrl_pressed = is_pressed || (!is_released && right_ctrl_pressed);
                    }
                }
                _ => {}
            }
        }

        if left_ctrl_pressed && right_ctrl_pressed {
            let started = hold_started_at.get_or_insert_with(Instant::now);
            if started.elapsed() >= HOLD_DURATION && !trigger_sent {
                log_launch("failsafe hotkey triggered");
                let _ = sender.send(());
                trigger_sent = true;
            }
        } else {
            hold_started_at = None;
            trigger_sent = false;
        }

        thread::sleep(POLL_INTERVAL);
    }
}
