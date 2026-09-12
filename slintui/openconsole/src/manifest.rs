use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_MANIFEST_PATH: &str = "/data/games/project/openconsole/games.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameManifest {
    pub games: Vec<GameEntry>,
    #[serde(default, rename = "all-games")]
    pub all_games: Vec<GameReference>,
    #[serde(default, rename = "top-games")]
    pub top_games: Vec<GameReference>,
    #[serde(default, rename = "new-games")]
    pub new_games: Vec<GameReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameReference {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameEntry {
    pub id: String,
    pub title: String,
    #[serde(default = "default_show_title")]
    pub showtitle: bool,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub developer: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub screenshots: Vec<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub env_remove: Vec<String>,
}

fn default_show_title() -> bool {
    true
}

impl GameManifest {
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|error| format!("failed to read manifest {}: {error}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|error| format!("failed to parse manifest {}: {error}", path.display()))
    }

    pub fn load_from_default_locations() -> Result<Self, String> {
        for path in candidate_manifest_paths() {
            if path.is_file() {
                return Self::load(&path);
            }
        }

        Err(format!(
            "no games manifest found; checked: {}",
            candidate_manifest_paths()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    pub fn game_by_index(&self, index: usize) -> Option<&GameEntry> {
        self.games.get(index)
    }

    pub fn game_by_id(&self, id: &str) -> Option<&GameEntry> {
        self.games.iter().find(|game| game.id == id)
    }
}

pub fn candidate_manifest_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(path) = std::env::var("OPENCONSOLE_GAMES_JSON") {
        paths.push(PathBuf::from(path));
    }

    paths.push(PathBuf::from(DEFAULT_MANIFEST_PATH));

    if let Ok(executable_path) = std::env::current_exe() {
        if let Some(parent) = executable_path.parent() {
            paths.push(parent.join("games.json"));
        }
    }

    if let Ok(current_dir) = std::env::current_dir() {
        paths.push(current_dir.join("games.json"));
    }

    paths
}
