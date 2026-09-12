use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_GAME_STATE_PATH: &str = "/data/openconsole/game-state.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameState {
    #[serde(default)]
    pub favorite_game_ids: Vec<String>,
    #[serde(default)]
    pub recently_played_game_ids: Vec<String>,
}

pub fn game_state_path() -> PathBuf {
    std::env::var("OPENCONSOLE_GAME_STATE_JSON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_GAME_STATE_PATH))
}

pub fn load_game_state() -> Result<GameState, String> {
    let path = game_state_path();
    if !path.exists() {
        return Ok(GameState::default());
    }

    let content = fs::read_to_string(&path).map_err(|error| {
        format!("failed to read game state {}: {error}", path.display())
    })?;

    serde_json::from_str(&content).map_err(|error| {
        format!("failed to parse game state {}: {error}", path.display())
    })
}

pub fn save_game_state(state: &GameState) -> Result<(), String> {
    let path = game_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("failed to create game state directory {}: {error}", parent.display())
        })?;
    }

    let payload = serde_json::to_string_pretty(state)
        .map_err(|error| format!("failed to serialize game state: {error}"))?;

    fs::write(&path, payload).map_err(|error| {
        format!("failed to write game state {}: {error}", path.display())
    })
}

impl GameState {
    pub fn is_favorite(&self, game_id: &str) -> bool {
        self.favorite_game_ids.iter().any(|candidate| candidate == game_id)
    }

    pub fn toggle_favorite(&mut self, game_id: &str) -> bool {
        if let Some(position) = self
            .favorite_game_ids
            .iter()
            .position(|candidate| candidate == game_id)
        {
            self.favorite_game_ids.remove(position);
            false
        } else {
            self.favorite_game_ids.push(game_id.to_string());
            true
        }
    }

    pub fn record_recently_played(&mut self, game_id: &str) {
        self.recently_played_game_ids
            .retain(|candidate| candidate != game_id);
        self.recently_played_game_ids.insert(0, game_id.to_string());
        self.recently_played_game_ids.truncate(10);
    }
}
