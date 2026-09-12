use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::controller::ControllerProfile;
use crate::controller_input::{
    list_all_controller_input_devices, list_controller_input_devices, ControllerInputDeviceInfo,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerSetupPhase {
    SelectDevice,
    MapControls,
    Review,
}

#[derive(Debug, Clone)]
pub struct ControllerSetupState {
    template: ControllerTemplate,
    phase: ControllerSetupPhase,
    devices: Vec<ControllerInputDeviceInfo>,
    selected_device_index: usize,
    bluetooth_address: Option<String>,
    index: usize,
    armed: bool,
    current_token: Option<String>,
    current_hits: usize,
    rotation_played: bool,
    mappings: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, Copy)]
enum ControllerTemplate {
    NesClassic,
    Snes,
    DualSense,
}

impl ControllerTemplate {
    fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Snes,
            2 => Self::DualSense,
            _ => Self::NesClassic,
        }
    }

    fn uses_all_devices(self) -> bool {
        matches!(self, Self::DualSense)
    }

    fn id(self) -> &'static str {
        match self {
            Self::NesClassic => "usb-nes-classic",
            Self::Snes => "usb-snes",
            Self::DualSense => "ps5-dualsense",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::NesClassic => "NES USB Controller",
            Self::Snes => "SNES USB Controller",
            Self::DualSense => "PS5 DualSense Controller",
        }
    }

    fn default_name(self, number: usize) -> String {
        match self {
            Self::NesClassic => format!("NES Controller #{number}"),
            Self::Snes => format!("SNES Controller #{number}"),
            Self::DualSense => format!("DualSense Controller #{number}"),
        }
    }

    fn actions(self) -> Vec<String> {
        match self {
            Self::DualSense => vec![
                "Move Up".to_string(),
                "Move Left".to_string(),
                "Move Right".to_string(),
                "Move Down".to_string(),
                "Button Cross (Select / Jump)".to_string(),
                "Button Circle (Back / Return)".to_string(),
                "Button Square".to_string(),
                "Button Triangle".to_string(),
                "Button Create (Secondary)".to_string(),
                "Button Options (Pause / Menu)".to_string(),
                "Button Touchpad (Secondary)".to_string(),
                "Button Mute".to_string(),
                "Button L1".to_string(),
                "Button L2".to_string(),
                "Button R1".to_string(),
                "Button R2".to_string(),
            ],
            Self::NesClassic => vec![
                "Move Up".to_string(),
                "Move Down".to_string(),
                "Move Left".to_string(),
                "Move Right".to_string(),
                "Select (Secondary)".to_string(),
                "Start (Pause / Menu)".to_string(),
                "Button B (Back / Return)".to_string(),
                "Button A (Jump / Select)".to_string(),
            ],
            Self::Snes => vec![
                "Move Up".to_string(),
                "Move Down".to_string(),
                "Move Left".to_string(),
                "Move Right".to_string(),
                "Select (Secondary)".to_string(),
                "Start (Pause / Menu)".to_string(),
                "Top Left (Secondary Select)".to_string(),
                "Top Right (Secondary Start)".to_string(),
                "Button Y (Back / Return)".to_string(),
                "Button X (Jump / Select)".to_string(),
                "Button B (Back / Return)".to_string(),
                "Button A (Jump / Select)".to_string(),
            ],
        }
    }

    fn confirm_action_prefix(self) -> &'static str {
        match self {
            Self::DualSense => "Button Cross",
            _ => "Button A",
        }
    }

    fn cancel_action_prefix(self) -> &'static str {
        match self {
            Self::DualSense => "Button Circle",
            _ => "Button B",
        }
    }
}

impl ControllerSetupState {
    pub fn new(template_index: i32) -> Self {
        let template = ControllerTemplate::from_index(template_index);
        let devices = if template.uses_all_devices() {
            list_all_controller_input_devices()
        } else {
            list_controller_input_devices()
        };
        let mappings = template
            .actions()
            .into_iter()
            .map(|action| (action, None))
            .collect();

        Self {
            template,
            phase: ControllerSetupPhase::SelectDevice,
            devices,
            selected_device_index: 0,
            bluetooth_address: None,
            index: 0,
            armed: false,
            current_token: None,
            current_hits: 0,
            rotation_played: false,
            mappings,
        }
    }

    pub fn title(&self) -> String {
        match self.phase {
            ControllerSetupPhase::SelectDevice => {
                if self.template.uses_all_devices() {
                    "Select Bluetooth Controller Device".to_string()
                } else {
                    "Select USB Controller Device".to_string()
                }
            }
            _ => self.template.title().to_string(),
        }
    }

    pub fn progress_text(&self) -> String {
        match self.phase {
            ControllerSetupPhase::SelectDevice => {
                if self.devices.is_empty() {
                    "0/0".to_string()
                } else {
                    format!("{}/{}", self.selected_device_index + 1, self.devices.len())
                }
            }
            ControllerSetupPhase::MapControls | ControllerSetupPhase::Review => {
                format!("{}/{}", self.completed_count(), self.mappings.len())
            }
        }
    }

    pub fn mapping_lines(&self) -> String {
        if self.phase == ControllerSetupPhase::SelectDevice {
            return self.device_list_text();
        }

        self.mappings
            .iter()
            .enumerate()
            .map(|(idx, (action, key))| {
                let mapped = key.clone().unwrap_or_else(|| "Not set".to_string());
                format!("{}. {} -> {}", idx + 1, action, mapped)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn instruction_text(&self) -> String {
        match self.phase {
            ControllerSetupPhase::SelectDevice => {
                if self.devices.is_empty() {
                    if self.template.uses_all_devices() {
                        "No Bluetooth controller devices found. Press ESC to go back.".to_string()
                    } else {
                        "No USB controller devices found. Press ESC to go back.".to_string()
                    }
                } else if self.template.uses_all_devices() {
                    "Use Up/Down to choose the Bluetooth controller, then press Enter to start mapping.".to_string()
                } else {
                    "Use Up/Down to choose the USB controller, then press Enter to start mapping.".to_string()
                }
            }
            ControllerSetupPhase::MapControls => {
                if self.is_complete() {
                    return "All controls captured. Press Enter to save.".to_string();
                }

                let action = &self.mappings[self.index].0;
                format!("Press the highlighted {action} button 10 times.")
            }
            ControllerSetupPhase::Review => {
                "All controls captured. Press Enter to save.".to_string()
            }
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.phase, ControllerSetupPhase::Review)
    }

    pub fn arm_next(&mut self) {
        if self.phase == ControllerSetupPhase::MapControls && !self.is_complete() {
            self.armed = true;
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    pub fn current_action(&self) -> Option<String> {
        if self.phase != ControllerSetupPhase::MapControls || self.index >= self.mappings.len() {
            None
        } else {
            Some(self.mappings[self.index].0.clone())
        }
    }

    pub fn artwork_stage(&self) -> i32 {
        if self.template.uses_all_devices() && self.rotation_played {
            18
        } else {
            0
        }
    }

    pub fn should_play_rotation_animation(&self) -> bool {
        self.template.uses_all_devices()
            && self.phase == ControllerSetupPhase::MapControls
            && !self.rotation_played
            && self.current_action().as_deref() == Some("Button L1")
    }

    pub fn mark_rotation_played(&mut self) {
        self.rotation_played = true;
    }

    pub fn has_played_rotation(&self) -> bool {
        self.rotation_played
    }

    pub fn device_list_text(&self) -> String {
        if self.devices.is_empty() {
            return "No SDL controller devices found.".to_string();
        }

        self.device_labels()
            .into_iter()
            .enumerate()
            .map(|(idx, label)| {
                let marker = if idx == self.selected_device_index {
                    ">"
                } else {
                    " "
                };
                format!("{marker} {label}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn device_labels(&self) -> Vec<String> {
        self.devices
            .iter()
            .map(Self::format_controller_device_label)
            .collect()
    }

    pub fn select_previous_device(&mut self) {
        if self.phase != ControllerSetupPhase::SelectDevice || self.devices.is_empty() {
            return;
        }

        if self.selected_device_index == 0 {
            self.selected_device_index = self.devices.len() - 1;
        } else {
            self.selected_device_index -= 1;
        }
    }

    pub fn select_next_device(&mut self) {
        if self.phase != ControllerSetupPhase::SelectDevice || self.devices.is_empty() {
            return;
        }

        self.selected_device_index = (self.selected_device_index + 1) % self.devices.len();
    }

    pub fn confirm_selected_device(&mut self) -> Option<ControllerInputDeviceInfo> {
        if self.phase != ControllerSetupPhase::SelectDevice {
            return self.selected_device_info().cloned();
        }

        let selected = self.selected_device_info().cloned();
        if selected.is_none() {
            return None;
        }
        self.phase = ControllerSetupPhase::MapControls;
        self.index = 0;
        self.armed = true;
        self.current_token = None;
        self.current_hits = 0;
        selected
    }

    pub fn skip_device_selection_for_dualsense(&mut self) {
        if self.phase != ControllerSetupPhase::SelectDevice || !self.template.uses_all_devices() {
            return;
        }

        if let Some(index) = self
            .devices
            .iter()
            .position(|device| {
                let lower_name = device.name.to_ascii_lowercase();
                lower_name.contains("dualsense") || lower_name.contains("wireless controller")
            })
        {
            self.selected_device_index = index;
        }

        self.phase = ControllerSetupPhase::MapControls;
        self.index = 0;
        self.armed = true;
        self.current_token = None;
        self.current_hits = 0;
    }

    pub fn selected_device_info(&self) -> Option<&ControllerInputDeviceInfo> {
        self.devices.get(self.selected_device_index)
    }

    pub fn selected_device_id(&self) -> Option<String> {
        self.selected_device_info()
            .map(|device| device.unique_id.clone())
    }

    pub fn selected_device_name(&self) -> Option<String> {
        self.selected_device_info().map(|device| device.name.clone())
    }

    pub fn selected_device_summary(&self) -> String {
        self.selected_device_info()
            .map(Self::format_controller_device_label)
            .unwrap_or_else(|| "No SDL controller devices found".to_string())
    }

    pub fn selected_device_index(&self) -> usize {
        self.selected_device_index
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }


fn format_controller_device_label(device: &ControllerInputDeviceInfo) -> String {
    let name = device.name.trim();
    let base_name = if name.is_empty() {
        "Controller"
    } else {
        name
    };

    if let Some(port) = Self::controller_device_port_hint(device) {
        format!("{base_name} - {port}")
    } else {
        base_name.to_string()
    }
}

fn controller_device_port_hint(device: &ControllerInputDeviceInfo) -> Option<String> {
    Self::extract_usb_port_from_physical_path(&device.physical_path)
        .map(|port| format!("USB port {port}"))
        .or_else(|| Self::extract_numeric_suffix(&device.event_node).map(|id| format!("USB ID {id}")))
        .or_else(|| Self::extract_instance_id(&device.physical_path).map(|id| format!("USB ID {id}")))
        .or_else(|| Self::extract_instance_id(&device.unique_id).map(|id| format!("USB ID {id}")))
}

fn extract_usb_port_from_physical_path(physical_path: &str) -> Option<String> {
    let lower = physical_path.to_ascii_lowercase();
    let start = lower.find("usb-")?;
    let tail = &physical_path[start + 4..];
    let segment = tail.split('/').next().unwrap_or(tail);
    let candidate = segment.rsplit('-').next().unwrap_or(segment).trim();
    if candidate.is_empty()
        || !candidate
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch == '.' || ch == '-')
    {
        return None;
    }

    Some(candidate.to_string())
}

fn extract_instance_id(text: &str) -> Option<String> {
    for marker in ["instance:", "instance ", "index "] {
        if let Some(start) = text.find(marker) {
            let tail = &text[start + marker.len()..];
            let digits = tail
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if !digits.is_empty() {
                return Some(digits);
            }
        }
    }

    None
}

fn extract_numeric_suffix(text: &str) -> Option<String> {
    let digits = text
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}
    pub fn is_device_selection(&self) -> bool {
        self.phase == ControllerSetupPhase::SelectDevice
    }

    pub fn record_key(&mut self, key: String) {
        if self.phase != ControllerSetupPhase::MapControls || !self.armed || self.is_complete() {
            return;
        }

        if let Some((_, mapped)) = self.mappings.get_mut(self.index) {
            *mapped = Some(key);
        }

        self.index += 1;
        self.armed = false;
        if self.index >= self.mappings.len() {
            self.phase = ControllerSetupPhase::Review;
        }
    }

    pub fn record_or_skip_current(&mut self, key: Option<String>) {
        if self.phase != ControllerSetupPhase::MapControls || self.is_complete() {
            return;
        }

        if let Some((_, mapped)) = self.mappings.get_mut(self.index) {
            *mapped = key;
        }

        self.index += 1;
        self.armed = false;
        if self.index >= self.mappings.len() {
            self.phase = ControllerSetupPhase::Review;
        }
    }

    pub fn register_mapping_press(&mut self, token: &str) -> bool {
        if self.phase != ControllerSetupPhase::MapControls || self.is_complete() {
            return false;
        }

        if self.current_token.as_deref() == Some(token) {
            self.current_hits += 1;
        } else {
            self.current_token = Some(token.to_string());
            self.current_hits = 1;
        }

        if self.current_hits < 10 {
            return false;
        }

        if let Some((_, mapped)) = self.mappings.get_mut(self.index) {
            *mapped = self.current_token.clone();
        }

        self.index += 1;
        self.current_token = None;
        self.current_hits = 0;
        self.armed = true;

        if self.index >= self.mappings.len() {
            self.phase = ControllerSetupPhase::Review;
            return true;
        }

        false
    }

    pub fn mapped_token_for_action_prefix(&self, prefix: &str) -> Option<String> {
        self.mappings
            .iter()
            .find(|(action, mapped)| action.starts_with(prefix) && mapped.is_some())
            .and_then(|(_, mapped)| mapped.clone())
    }

    pub fn confirm_action_prefix(&self) -> &'static str {
        self.template.confirm_action_prefix()
    }

    pub fn cancel_action_prefix(&self) -> &'static str {
        self.template.cancel_action_prefix()
    }

    pub fn set_bluetooth_address(&mut self, bluetooth_address: Option<String>) {
        self.bluetooth_address = bluetooth_address;
    }

    pub fn to_profile(&self, existing_count: usize) -> ControllerProfile {
        let mut mappings = BTreeMap::new();
        for (action, key) in &self.mappings {
            if let Some(key) = key {
                mappings.insert(action.clone(), key.clone());
            }
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        ControllerProfile {
            id: format!("{}-{timestamp}", self.template.id()),
            name: self.template.default_name(existing_count + 1),
            template: self.template.id().to_string(),
            device_id: self.selected_device_id(),
            device_name: self.selected_device_name(),
            bluetooth_address: self.bluetooth_address.clone(),
            mappings,
        }
    }

    fn completed_count(&self) -> usize {
        self.mappings
            .iter()
            .filter(|(_, key)| key.is_some())
            .count()
    }

    pub fn artwork_set(&self) -> &'static str {
        match self.template {
            ControllerTemplate::NesClassic => "nes",
            ControllerTemplate::Snes => "snes",
            ControllerTemplate::DualSense => "dualsense",
        }
    }

    pub fn current_button_label(&self) -> String {
        self.current_overlay_name()
            .map(|name| match name {
                "up" => "Up",
                "down" => "Down",
                "left" => "Left",
                "right" => "Right",
                "select" => "Select",
                "start" => "Start",
                "topleft" => "Top Left",
                "topright" => "Top Right",
                "cross" => "Cross",
                "circle" => "Circle",
                "square" => "Square",
                "triangle" => "Triangle",
                "touch_area" => "Touchpad",
                "mute" => "Mute",
                "l1" => "L1",
                "l2" => "L2",
                "r1" => "R1",
                "r2" => "R2",
                "y" => "Y",
                "x" => "X",
                "b" => "B",
                "a" => "A",
                _ => "",
            }
            .to_string())
            .unwrap_or_default()
    }

    pub fn current_overlay_name(&self) -> Option<&'static str> {
        let action = self.current_action()?;
        if action.starts_with("Move Up") {
            Some("up")
        } else if action.starts_with("Move Down") {
            Some("down")
        } else if action.starts_with("Move Left") {
            Some("left")
        } else if action.starts_with("Move Right") {
            Some("right")
        } else if action.starts_with("Button Cross") {
            Some("cross")
        } else if action.starts_with("Button Circle") {
            Some("circle")
        } else if action.starts_with("Button Square") {
            Some("square")
        } else if action.starts_with("Button Triangle") {
            Some("triangle")
        } else if action.starts_with("Button Create") {
            Some("select")
        } else if action.starts_with("Button Options") {
            Some("start")
        } else if action.starts_with("Button Touchpad") {
            Some("touch_area")
        } else if action.starts_with("Button Mute") {
            Some("mute")
        } else if action.starts_with("Button L1") {
            Some("l1")
        } else if action.starts_with("Button L2") {
            Some("l2")
        } else if action.starts_with("Button R1") {
            Some("r1")
        } else if action.starts_with("Button R2") {
            Some("r2")
        } else if action.starts_with("Select") {
            Some("select")
        } else if action.starts_with("Start") {
            Some("start")
        } else if action.starts_with("Top Left") {
            Some("topleft")
        } else if action.starts_with("Top Right") {
            Some("topright")
        } else if action.starts_with("Button Y") {
            Some("y")
        } else if action.starts_with("Button X") {
            Some("x")
        } else if action.starts_with("Button B") {
            Some("b")
        } else if action.starts_with("Button A") {
            Some("a")
        } else {
            None
        }
    }

    pub fn current_press_count(&self) -> usize {
        self.current_hits.min(10)
    }

    pub fn current_progress_percent(&self) -> i32 {
        if self.is_complete() {
            return 100;
        }

        (self.current_press_count() as i32) * 10
    }

    pub fn is_mapping_controls(&self) -> bool {
        self.phase == ControllerSetupPhase::MapControls
    }
}