use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use openconsole::bluetooth::{
    connect_dualsense_device, pair_dualsense_device, reconnect_saved_dualsense_devices,
    scan_dualsense_devices, unpair_dualsense_device,
    BluetoothDeviceInfo, BluetoothScanReport,
};
use openconsole::controller::{list_text_with_selection, load_store, save_store};
use openconsole::controller_input::list_all_controller_input_devices;
use openconsole::controller_setup::ControllerSetupState;
use openconsole::game_state::{game_state_path, load_game_state, save_game_state, GameState};
use openconsole::input::{
    start_console_input, ConsoleAction, ConsoleInputEvent, ConsoleInputPhase, ConsoleInputSource,
};
use openconsole::ipc::send_runtime_mapper_toggle_request;
use openconsole::ipc::{send_launch_request, send_reboot_request, supervisor_socket_path};
use openconsole::logging::log_launch;
use openconsole::manifest::{candidate_manifest_paths, GameManifest};
use openconsole::system_settings::{
    apply_system_settings, load_or_create_system_settings, save_system_settings, SystemSettings,
};
use openconsole::wifi::{
    connect_to_network, disconnect_wifi, enforce_connectivity_policy, read_status, refresh_network_status,
    scan_networks, WifiNetwork,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, TimerMode, VecModel};

slint::include_modules!();

#[derive(Clone)]
struct RuntimeGameCatalog {
    manifest: GameManifest,
    manifest_path: PathBuf,
}

#[derive(Default)]
struct GameDetailImageCache {
    covers: HashMap<usize, slint::Image>,
    screenshots: HashMap<usize, Vec<slint::Image>>,
}

#[derive(Clone, Copy)]
enum GameImageVariant {
    Banner,
    Tile,
    Top,
    Thumbnail,
}

const WIFI_NETWORK_WINDOW_SIZE: usize = 8;
const BLUETOOTH_DEBUG_VISIBLE_LINES: usize = 10;
const BLUETOOTH_MAPPING_ENUMERATION_RETRY_COUNT: usize = 8;
const BLUETOOTH_MAPPING_ENUMERATION_RETRY_DELAY: Duration = Duration::from_millis(500);
const WIFI_NETWORK_STATE_NONE: i32 = 0;
const WIFI_NETWORK_STATE_CONNECTED: i32 = 1;
const WIFI_NETWORK_STATE_UNABLE_TO_CONNECT: i32 = 2;
const WIFI_PASSWORD_ACTION_KEYS: i32 = 0;
const WIFI_PASSWORD_ACTION_VISIBILITY: i32 = 1;
const WIFI_PASSWORD_ACTION_MODE_UPPER: i32 = 2;
const WIFI_PASSWORD_ACTION_MODE_LOWER: i32 = 3;
const WIFI_PASSWORD_ACTION_MODE_SYMBOLS: i32 = 4;
const WIFI_PASSWORD_ACTION_CONNECT: i32 = 5;
const BLUETOOTH_ACTION_PRIMARY_BUTTON: i32 = 0;
const BLUETOOTH_ACTION_DEVICE_LIST: i32 = 1;
const HOME_SECTION_FAVORITES: i32 = 0;
const HOME_SECTION_RECENT: i32 = 1;
const GAME_SECTION_NEW: i32 = 0;
const GAME_SECTION_TOP: i32 = 1;
const GAME_SECTION_ALL: i32 = 2;
const WIFI_PASSWORD_REGULAR_ROW_0: [&str; 12] = [
    "Space", "q", "w", "e", "r", "t", "y", "u", "i", "o", "p", "Delete",
];
const WIFI_PASSWORD_REGULAR_ROW_1: [&str; 9] = ["a", "s", "d", "f", "g", "h", "j", "k", "l"];
const WIFI_PASSWORD_REGULAR_ROW_2: [&str; 7] = ["z", "x", "c", "v", "b", "n", "m"];
const WIFI_PASSWORD_CAPITAL_ROW_0: [&str; 12] = [
    "Space", "Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "Delete",
];
const WIFI_PASSWORD_CAPITAL_ROW_1: [&str; 9] = ["A", "S", "D", "F", "G", "H", "J", "K", "L"];
const WIFI_PASSWORD_CAPITAL_ROW_2: [&str; 7] = ["Z", "X", "C", "V", "B", "N", "M"];
const WIFI_PASSWORD_SPECIFIC_ROW_0: [&str; 12] = [
    "Space", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "Delete",
];

thread_local! {
    static BLUETOOTH_PAIRING_LIGHT_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}
const WIFI_PASSWORD_SPECIFIC_ROW_1: [&str; 10] = ["-", "/", ":", ";", "(", ")", "$", "&", "@", "\\"];
const WIFI_PASSWORD_SPECIFIC_ROW_2: [&str; 6] = [".", ",", "?", "!", "'", "#"];
const WIFI_PASSWORD_REGULAR_ROWS: [&[&str]; 3] = [
    &WIFI_PASSWORD_REGULAR_ROW_0,
    &WIFI_PASSWORD_REGULAR_ROW_1,
    &WIFI_PASSWORD_REGULAR_ROW_2,
];
const WIFI_PASSWORD_CAPITAL_ROWS: [&[&str]; 3] = [
    &WIFI_PASSWORD_CAPITAL_ROW_0,
    &WIFI_PASSWORD_CAPITAL_ROW_1,
    &WIFI_PASSWORD_CAPITAL_ROW_2,
];
const WIFI_PASSWORD_SPECIFIC_ROWS: [&[&str]; 3] = [
    &WIFI_PASSWORD_SPECIFIC_ROW_0,
    &WIFI_PASSWORD_SPECIFIC_ROW_1,
    &WIFI_PASSWORD_SPECIFIC_ROW_2,
];

fn load_manifest_for_ui() -> Result<(GameManifest, PathBuf), String> {
    for path in candidate_manifest_paths() {
        if path.is_file() {
            let manifest = GameManifest::load(&path)?;
            return Ok((manifest, path));
        }
    }

    Err("no games manifest file found for UI".to_string())
}

fn resolve_image_path(manifest_path: &Path, image: &str) -> PathBuf {
    let image_path = PathBuf::from(image);
    if image_path.is_absolute() {
        return image_path;
    }

    manifest_path
        .parent()
        .map(|parent| parent.join(&image_path))
        .unwrap_or(image_path)
}

fn resolve_image_variant_path(
    manifest_path: &Path,
    image: &str,
    variant: GameImageVariant,
) -> PathBuf {
    let original_path = resolve_image_path(manifest_path, image);

    let variant_suffixes: &[&str] = match variant {
        GameImageVariant::Banner => &["-banner"],
        GameImageVariant::Tile => &["-tile", "-thumb"],
        GameImageVariant::Top => &["-top", "-tile", "-thumb"],
        GameImageVariant::Thumbnail => &["-thumb", "-tile"],
    };

    if let Some(parent) = original_path.parent() {
        if let Some(stem) = original_path.file_stem().and_then(|stem| stem.to_str()) {
            for suffix in variant_suffixes {
                for extension in ["jpg", "jpeg", "png"] {
                    let candidate = parent.join(format!("{stem}{suffix}.{extension}"));
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }
    }

    original_path
}

fn load_game_image(
    manifest_path: &Path,
    image: &str,
    variant: GameImageVariant,
    context: &str,
) -> slint::Image {
    let image_path = resolve_image_variant_path(manifest_path, image, variant);
    match slint::Image::load_from_path(&image_path) {
        Ok(image) => image,
        Err(error) => {
            log_launch(&format!(
                "ui failed to load {context} image {}: {error}",
                image_path.display()
            ));
            slint::Image::default()
        }
    }
}

fn load_runtime_game_catalog(app: &Demo) -> Result<RuntimeGameCatalog, String> {
    let (manifest, manifest_path) = if app.get_development_build() {
        load_placeholder_manifest()
    } else {
        load_manifest_for_ui().unwrap_or_else(|error| {
            log_launch(&format!("ui manifest load failed; using placeholder game data: {error}"));
            load_placeholder_manifest()
        })
    };

    Ok(RuntimeGameCatalog {
        manifest,
        manifest_path,
    })
}

fn resolve_game_images(manifest_path: &Path, game: &openconsole::manifest::GameEntry) -> Vec<slint::Image> {
    let mut images = Vec::with_capacity(game.screenshots.len().max(1));
    if game.screenshots.is_empty() {
        if !game.image.is_empty() {
            images.push(load_game_image(
                manifest_path,
                &game.image,
                GameImageVariant::Thumbnail,
                "screenshot fallback",
            ));
        }
        if images.is_empty() {
            images.push(slint::Image::default());
        }
        return images;
    }

    for screenshot in &game.screenshots {
        images.push(load_game_image(
            manifest_path,
            screenshot,
            GameImageVariant::Thumbnail,
            "screenshot",
        ));
    }

    images
}

fn resolve_game_section(
    catalog: &RuntimeGameCatalog,
    references: &[openconsole::manifest::GameReference],
    cover_variant: GameImageVariant,
) -> (Vec<SharedString>, Vec<SharedString>, Vec<bool>, Vec<slint::Image>, Vec<i32>) {
    let mut titles: Vec<SharedString> = Vec::with_capacity(references.len());
    let mut subtitles: Vec<SharedString> = Vec::with_capacity(references.len());
    let mut show_titles: Vec<bool> = Vec::with_capacity(references.len());
    let mut covers: Vec<slint::Image> = Vec::with_capacity(references.len());
    let mut indices: Vec<i32> = Vec::with_capacity(references.len());

    for reference in references {
        if let Some((catalog_index, entry)) = catalog
            .manifest
            .games
            .iter()
            .enumerate()
            .find(|(_, game)| game.id == reference.id)
        {
            indices.push(catalog_index as i32);
            titles.push(entry.title.clone().into());
            subtitles.push(entry.subtitle.clone().into());
            show_titles.push(entry.showtitle);

            if !entry.image.is_empty() {
                covers.push(load_game_image(
                    &catalog.manifest_path,
                    &entry.image,
                    cover_variant,
                    "section cover",
                ));
            } else {
                covers.push(slint::Image::default());
            }
        }
    }

    (titles, subtitles, show_titles, covers, indices)
}

fn load_placeholder_manifest() -> (GameManifest, PathBuf) {
    let manifest = GameManifest {
        games: vec![
            openconsole::manifest::GameEntry {
                id: "pirate-puzzle".to_string(),
                title: "Pirate".to_string(),
                showtitle: false,
                subtitle: "Puzzle | Installed".to_string(),
                description: "Embark on a thrilling pirate adventure, solving puzzles and uncovering hidden treasures.".to_string(),
                developer: "PlayRoot".to_string(),
                image: "assets/games/2/game.jpg".to_string(),
                screenshots: vec![
                    "assets/games/2/screenshots/1.jpg".to_string(),
                    "assets/games/2/screenshots/2.jpg".to_string(),
                    "assets/games/2/screenshots/3.jpg".to_string(),
                    "assets/games/2/screenshots/4.jpg".to_string(),
                ],
                command: "/data/games/project/openconsole/run-game-as-weston.sh".to_string(),
                args: vec![
                    "--display-driver".to_string(),
                    "wayland".to_string(),
                    "--rendering-driver".to_string(),
                    "opengl3".to_string(),
                    "--fullscreen".to_string(),
                    "--disable-vsync".to_string(),
                    "--max-fps".to_string(),
                    "60".to_string(),
                ],
                cwd: Some("/data/games/project/openconsole".to_string()),
                env: [
                    ("OPENCONSOLE_GAME_BIN".to_string(), "pirate.arm64".to_string()),
                    ("XDG_RUNTIME_DIR".to_string(), "/run".to_string()),
                    ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()),
                ]
                .into_iter()
                .collect(),
                env_remove: vec!["DISPLAY".to_string(), "DRI_PRIME".to_string(), "SLINT_BACKEND".to_string()],
            },
            openconsole::manifest::GameEntry {
                id: "pizza-delivery".to_string(),
                title: "Pizza Delivery".to_string(),
                showtitle: false,
                subtitle: "Strategy | Installed".to_string(),
                description: "Deliver pizzas efficiently while managing obstacles and time constraints.".to_string(),
                developer: "PlayRoot".to_string(),
                image: "assets/games/3/game.jpg".to_string(),
                screenshots: vec![
                    "assets/games/3/screenshots/1.jpg".to_string(),
                    "assets/games/3/screenshots/2.jpg".to_string(),
                    "assets/games/3/screenshots/3.jpg".to_string(),
                    "assets/games/3/screenshots/4.jpg".to_string(),
                ],
                command: "/data/games/project/openconsole/run-game-as-weston.sh".to_string(),
                args: vec![
                    "--display-driver".to_string(),
                    "wayland".to_string(),
                    "--rendering-driver".to_string(),
                    "opengl3".to_string(),
                    "--fullscreen".to_string(),
                    "--disable-vsync".to_string(),
                    "--max-fps".to_string(),
                    "60".to_string(),
                ],
                cwd: Some("/data/games/project/openconsole".to_string()),
                env: [
                    ("OPENCONSOLE_GAME_BIN".to_string(), "pizzadelivery.arm64".to_string()),
                    ("XDG_RUNTIME_DIR".to_string(), "/run".to_string()),
                    ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()),
                ]
                .into_iter()
                .collect(),
                env_remove: vec!["DISPLAY".to_string(), "DRI_PRIME".to_string(), "SLINT_BACKEND".to_string()],
            },
            openconsole::manifest::GameEntry {
                id: "soko-bloom".to_string(),
                title: "Soko Bloom".to_string(),
                showtitle: false,
                subtitle: "Puzzle | Installed".to_string(),
                description: "Solve challenging puzzles in a vibrant and colorful world.".to_string(),
                developer: "PlayRoot".to_string(),
                image: "assets/games/4/game.jpg".to_string(),
                screenshots: vec![
                    "assets/games/4/screenshots/1.jpg".to_string(),
                    "assets/games/4/screenshots/2.jpg".to_string(),
                    "assets/games/4/screenshots/3.jpg".to_string(),
                    "assets/games/4/screenshots/4.jpg".to_string(),
                ],
                command: "/data/games/project/openconsole/run-game-as-weston.sh".to_string(),
                args: vec![
                    "--display-driver".to_string(),
                    "wayland".to_string(),
                    "--rendering-driver".to_string(),
                    "opengl3".to_string(),
                    "--fullscreen".to_string(),
                    "--disable-vsync".to_string(),
                    "--max-fps".to_string(),
                    "60".to_string(),
                ],
                cwd: Some("/data/games/project/openconsole".to_string()),
                env: [
                    ("OPENCONSOLE_GAME_BIN".to_string(), "puzzle.arm64".to_string()),
                    ("XDG_RUNTIME_DIR".to_string(), "/run".to_string()),
                    ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()),
                ]
                .into_iter()
                .collect(),
                env_remove: vec!["DISPLAY".to_string(), "DRI_PRIME".to_string(), "SLINT_BACKEND".to_string()],
            },
            openconsole::manifest::GameEntry {
                id: "hogbound".to_string(),
                title: "Hogbound".to_string(),
                showtitle: false,
                subtitle: "Platformer | Installed".to_string(),
                description: "Navigate through tricky platforming levels and overcome various challenges.".to_string(),
                developer: "PlayRoot".to_string(),
                image: "assets/games/1/game.jpg".to_string(),
                screenshots: vec![
                    "assets/games/1/screenshots/1.jpg".to_string(),
                    "assets/games/1/screenshots/2.jpg".to_string(),
                    "assets/games/1/screenshots/3.jpg".to_string(),
                    "assets/games/1/screenshots/4.jpg".to_string(),
                ],
                command: "/data/games/project/openconsole/run-game-as-weston.sh".to_string(),
                args: vec![
                    "--display-driver".to_string(),
                    "wayland".to_string(),
                    "--rendering-driver".to_string(),
                    "opengl3".to_string(),
                    "--fullscreen".to_string(),
                    "--disable-vsync".to_string(),
                    "--max-fps".to_string(),
                    "60".to_string(),
                ],
                cwd: Some("/data/games/project/openconsole".to_string()),
                env: [
                    ("OPENCONSOLE_GAME_BIN".to_string(), "platform.arm64".to_string()),
                    ("XDG_RUNTIME_DIR".to_string(), "/run".to_string()),
                    ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()),
                ]
                .into_iter()
                .collect(),
                env_remove: vec!["DISPLAY".to_string(), "DRI_PRIME".to_string(), "SLINT_BACKEND".to_string()],
            },
        ],
        all_games: vec![
            openconsole::manifest::GameReference { id: "soko-bloom".to_string() },
            openconsole::manifest::GameReference { id: "hogbound".to_string() },
            openconsole::manifest::GameReference { id: "pirate-puzzle".to_string() },
            openconsole::manifest::GameReference { id: "pizza-delivery".to_string() },
            openconsole::manifest::GameReference { id: "soko-bloom".to_string() },
            openconsole::manifest::GameReference { id: "hogbound".to_string() },
            openconsole::manifest::GameReference { id: "pirate-puzzle".to_string() },
        ],
        top_games: vec![
            openconsole::manifest::GameReference { id: "pirate-puzzle".to_string() },
            openconsole::manifest::GameReference { id: "soko-bloom".to_string() },
            openconsole::manifest::GameReference { id: "hogbound".to_string() },
        ],
        new_games: vec![
            openconsole::manifest::GameReference { id: "pizza-delivery".to_string() },
            openconsole::manifest::GameReference { id: "soko-bloom".to_string() },
            openconsole::manifest::GameReference { id: "hogbound".to_string() },
            openconsole::manifest::GameReference { id: "pirate-puzzle".to_string() },
            openconsole::manifest::GameReference { id: "pizza-delivery".to_string() },
            openconsole::manifest::GameReference { id: "soko-bloom".to_string() },
            openconsole::manifest::GameReference { id: "hogbound".to_string() },
            openconsole::manifest::GameReference { id: "pirate-puzzle".to_string() },
        ],
    };

    let manifest_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    (manifest, manifest_path)
}

fn apply_manifest_to_ui(app: &Demo, catalog: &RuntimeGameCatalog) {
    let manifest = &catalog.manifest;
    let manifest_path = &catalog.manifest_path;

    let game_count = manifest.games.len();
    app.set_game_count(game_count as i32);
    if app.get_selected_game() >= game_count as i32 {
        app.set_selected_game(0);
    }

    let mut titles: Vec<SharedString> = Vec::with_capacity(game_count);
    let mut subtitles: Vec<SharedString> = Vec::with_capacity(game_count);
    let mut descriptions: Vec<SharedString> = Vec::with_capacity(game_count);
    let mut developers: Vec<SharedString> = Vec::with_capacity(game_count);
    let mut show_titles: Vec<bool> = Vec::with_capacity(game_count);

    for idx in 0..game_count {
        let entry = &manifest.games[idx];
        titles.push(entry.title.clone().into());
        subtitles.push(entry.subtitle.clone().into());
        descriptions.push(entry.description.clone().into());
        developers.push(entry.developer.clone().into());
        show_titles.push(entry.showtitle);
    }

    app.set_game_titles(ModelRc::new(VecModel::from(titles)));
    app.set_game_subtitles(ModelRc::new(VecModel::from(subtitles)));
    app.set_game_descriptions(ModelRc::new(VecModel::from(descriptions)));
    app.set_game_developers(ModelRc::new(VecModel::from(developers)));
    app.set_game_show_titles(ModelRc::new(VecModel::from(show_titles)));

    let resolve_section = |references: &[openconsole::manifest::GameReference], cover_variant| {
        let mut section_titles: Vec<SharedString> = Vec::with_capacity(references.len());
        let mut section_subtitles: Vec<SharedString> = Vec::with_capacity(references.len());
        let mut section_show_titles: Vec<bool> = Vec::with_capacity(references.len());
        let mut section_covers: Vec<slint::Image> = Vec::with_capacity(references.len());
        let mut section_indices: Vec<i32> = Vec::with_capacity(references.len());

        for reference in references {
            if let Some((catalog_index, entry)) = manifest
                .games
                .iter()
                .enumerate()
                .find(|(_, game)| game.id == reference.id)
            {
                section_indices.push(catalog_index as i32);
                section_titles.push(entry.title.clone().into());
                section_subtitles.push(entry.subtitle.clone().into());
                section_show_titles.push(entry.showtitle);

                if !entry.image.is_empty() {
                    section_covers.push(load_game_image(
                        &manifest_path,
                        &entry.image,
                        cover_variant,
                        "games row cover",
                    ));
                } else {
                    section_covers.push(slint::Image::default());
                }
            }
        }

        (section_titles, section_subtitles, section_show_titles, section_covers, section_indices)
    };

    let all_games = if manifest.all_games.is_empty() {
        manifest
            .games
            .iter()
            .map(|game| openconsole::manifest::GameReference { id: game.id.clone() })
            .collect::<Vec<_>>()
    } else {
        manifest.all_games.clone()
    };

    let (new_titles, new_subtitles, new_show_titles, new_covers, new_indices) =
        resolve_section(&manifest.new_games, GameImageVariant::Tile);
    let (top_titles, top_subtitles, top_show_titles, top_covers, top_indices) =
        resolve_section(&manifest.top_games, GameImageVariant::Top);
    let (all_titles, all_subtitles, all_show_titles, all_covers, all_indices) =
        resolve_section(&all_games, GameImageVariant::Tile);

    app.set_new_game_titles(ModelRc::new(VecModel::from(new_titles)));
    app.set_new_game_subtitles(ModelRc::new(VecModel::from(new_subtitles)));
    app.set_new_game_show_titles(ModelRc::new(VecModel::from(new_show_titles)));
    app.set_new_game_covers(ModelRc::new(VecModel::from(new_covers)));
    app.set_new_game_indices(ModelRc::new(VecModel::from(new_indices)));
    app.set_new_game_selected_index(0);

    app.set_top_game_titles(ModelRc::new(VecModel::from(top_titles)));
    app.set_top_game_subtitles(ModelRc::new(VecModel::from(top_subtitles)));
    app.set_top_game_show_titles(ModelRc::new(VecModel::from(top_show_titles)));
    app.set_top_game_covers(ModelRc::new(VecModel::from(top_covers)));
    app.set_top_game_indices(ModelRc::new(VecModel::from(top_indices)));
    app.set_top_game_selected_index(0);

    app.set_all_game_titles(ModelRc::new(VecModel::from(all_titles)));
    app.set_all_game_subtitles(ModelRc::new(VecModel::from(all_subtitles)));
    app.set_all_game_show_titles(ModelRc::new(VecModel::from(all_show_titles)));
    app.set_all_game_covers(ModelRc::new(VecModel::from(all_covers)));
    app.set_all_game_indices(ModelRc::new(VecModel::from(all_indices)));
    app.set_all_game_selected_index(0);

    log_launch(&format!(
        "ui loaded {} games from {}",
        game_count,
        manifest_path.display()
    ));
}

fn refresh_home_view(app: &Demo, catalog: &RuntimeGameCatalog, game_state: &GameState) {
    let favorite_refs = game_state
        .favorite_game_ids
        .iter()
        .map(|id| openconsole::manifest::GameReference { id: id.clone() })
        .collect::<Vec<_>>();
    let recent_refs = game_state
        .recently_played_game_ids
        .iter()
        .map(|id| openconsole::manifest::GameReference { id: id.clone() })
        .collect::<Vec<_>>();

        let (favorite_titles, favorite_subtitles, favorite_show_titles, favorite_covers, favorite_indices) =
            resolve_game_section(catalog, &favorite_refs, GameImageVariant::Tile);
        let (recent_titles, recent_subtitles, recent_show_titles, recent_covers, recent_indices) =
            resolve_game_section(catalog, &recent_refs, GameImageVariant::Tile);

    app.set_favorite_game_titles(ModelRc::new(VecModel::from(favorite_titles)));
    app.set_favorite_game_subtitles(ModelRc::new(VecModel::from(favorite_subtitles)));
    app.set_favorite_game_show_titles(ModelRc::new(VecModel::from(favorite_show_titles)));
    app.set_favorite_game_covers(ModelRc::new(VecModel::from(favorite_covers)));
    app.set_favorite_game_indices(ModelRc::new(VecModel::from(favorite_indices)));

    app.set_recent_game_titles(ModelRc::new(VecModel::from(recent_titles)));
    app.set_recent_game_subtitles(ModelRc::new(VecModel::from(recent_subtitles)));
    app.set_recent_game_show_titles(ModelRc::new(VecModel::from(recent_show_titles)));
    app.set_recent_game_covers(ModelRc::new(VecModel::from(recent_covers)));
    app.set_recent_game_indices(ModelRc::new(VecModel::from(recent_indices)));

    if app.get_favorite_game_selected_index() >= app.get_favorite_game_indices().row_count() as i32 {
        app.set_favorite_game_selected_index(0);
    }
    if app.get_recent_game_selected_index() >= app.get_recent_game_indices().row_count() as i32 {
        app.set_recent_game_selected_index(0);
    }
}

fn refresh_selected_game_view(
    app: &Demo,
    catalog: &RuntimeGameCatalog,
    game_state: &GameState,
    detail_image_cache: &Arc<Mutex<GameDetailImageCache>>,
) {
    let selected_game = app.get_selected_game().max(0) as usize;
    if let Some(game) = catalog.manifest.game_by_index(selected_game) {
        app.set_game_favorite(game_state.is_favorite(&game.id));
        app.set_selected_screenshot_index(0);
        app.set_screenshot_popup_open(false);

        let cached_images = {
            let guard = detail_image_cache
                .lock()
                .expect("detail image cache lock poisoned");
            guard
                .covers
                .get(&selected_game)
                .cloned()
                .zip(guard.screenshots.get(&selected_game).cloned())
        };

        if let Some((cover, screenshots)) = cached_images {
            app.set_selected_game_cover(cover);
            app.set_selected_game_cover_loading(false);
            app.set_game_screenshots(ModelRc::new(VecModel::from(screenshots)));
            app.set_game_screenshots_loading(false);
            return;
        }

        app.set_selected_game_cover(slint::Image::default());
        app.set_selected_game_cover_loading(true);
        app.set_game_screenshots(ModelRc::new(VecModel::from(Vec::<slint::Image>::new())));
        app.set_game_screenshots_loading(true);

        let app_weak = app.as_weak();
        let manifest_path = catalog.manifest_path.clone();
        let game = game.clone();
        let detail_image_cache = Arc::clone(detail_image_cache);
        slint::Timer::single_shot(Duration::from_millis(16), move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            if app.get_selected_game().max(0) as usize != selected_game {
                return;
            }

            let cover = if game.image.is_empty() {
                slint::Image::default()
            } else {
                load_game_image(
                    &manifest_path,
                    &game.image,
                    GameImageVariant::Banner,
                    "game detail cover",
                )
            };
            let screenshots = resolve_game_images(&manifest_path, &game);

            {
                let mut guard = detail_image_cache
                    .lock()
                    .expect("detail image cache lock poisoned");
                guard.covers.insert(selected_game, cover.clone());
                guard.screenshots.insert(selected_game, screenshots.clone());
            }

            app.set_selected_game_cover(cover);
            app.set_selected_game_cover_loading(false);
            app.set_game_screenshots(ModelRc::new(VecModel::from(screenshots)));
            app.set_game_screenshots_loading(false);
        });
    } else {
        app.set_selected_game_cover(slint::Image::default());
        app.set_selected_game_cover_loading(false);
        app.set_game_screenshots(ModelRc::new(VecModel::from(Vec::<slint::Image>::new())));
        app.set_game_screenshots_loading(false);
        app.set_game_favorite(false);
        app.set_selected_screenshot_index(0);
        app.set_screenshot_popup_open(false);
    }
}

fn home_section_row_count(app: &Demo, section: i32) -> usize {
    match section {
        HOME_SECTION_FAVORITES => app.get_favorite_game_indices().row_count(),
        HOME_SECTION_RECENT => app.get_recent_game_indices().row_count(),
        _ => 0,
    }
}

fn home_section_selected_index(app: &Demo, section: i32) -> i32 {
    match section {
        HOME_SECTION_FAVORITES => app.get_favorite_game_selected_index(),
        HOME_SECTION_RECENT => app.get_recent_game_selected_index(),
        _ => 0,
    }
}

fn set_home_section_selected_index(app: &Demo, section: i32, index: i32) {
    match section {
        HOME_SECTION_FAVORITES => app.set_favorite_game_selected_index(index),
        HOME_SECTION_RECENT => app.set_recent_game_selected_index(index),
        _ => {}
    }
}

fn first_available_home_section(app: &Demo) -> Option<i32> {
    [HOME_SECTION_FAVORITES, HOME_SECTION_RECENT]
        .into_iter()
        .find(|section| home_section_row_count(app, *section) > 0)
}

fn normalize_home_selected_section(app: &Demo) -> Option<i32> {
    let current = app.get_home_selected_section().clamp(HOME_SECTION_FAVORITES, HOME_SECTION_RECENT);
    let section = if home_section_row_count(app, current) > 0 {
        Some(current)
    } else {
        first_available_home_section(app)
    };

    if let Some(section) = section {
        app.set_home_selected_section(section);
    }

    section
}

fn sync_selected_game_from_home_section(app: &Demo) {
    let Some(section) = normalize_home_selected_section(app) else {
        return;
    };

    let row_count = home_section_row_count(app, section);
    if row_count == 0 {
        return;
    }

    let selected_index = home_section_selected_index(app, section).clamp(0, row_count as i32 - 1);
    if selected_index != home_section_selected_index(app, section) {
        set_home_section_selected_index(app, section, selected_index);
    }

    let indices = match section {
        HOME_SECTION_FAVORITES => app.get_favorite_game_indices(),
        HOME_SECTION_RECENT => app.get_recent_game_indices(),
        _ => return,
    };

    if let Some(game_index) = indices.row_data(selected_index as usize) {
        app.invoke_select_game(game_index);
    }
}

fn move_home_section_vertical(app: &Demo, delta: i32) -> bool {
    let Some(current) = normalize_home_selected_section(app) else {
        return false;
    };

    let next = current + delta;
    if !(HOME_SECTION_FAVORITES..=HOME_SECTION_RECENT).contains(&next) {
        return false;
    }
    if home_section_row_count(app, next) == 0 {
        return false;
    }

    app.set_home_selected_section(next);
    sync_selected_game_from_home_section(app);
    true
}

fn move_home_section_horizontal(app: &Demo, delta: i32) -> bool {
    let Some(section) = normalize_home_selected_section(app) else {
        return false;
    };

    let row_count = home_section_row_count(app, section) as i32;
    if row_count <= 0 {
        return false;
    }

    let current = home_section_selected_index(app, section).clamp(0, row_count - 1);
    let next = (current + delta).clamp(0, row_count - 1);
    if next == current {
        return false;
    }

    set_home_section_selected_index(app, section, next);
    sync_selected_game_from_home_section(app);
    true
}

fn game_section_row_count(app: &Demo, section: i32) -> usize {
    match section {
        GAME_SECTION_NEW => app.get_new_game_indices().row_count(),
        GAME_SECTION_TOP => app.get_top_game_indices().row_count(),
        GAME_SECTION_ALL => app.get_all_game_indices().row_count(),
        _ => 0,
    }
}

fn game_section_selected_index(app: &Demo, section: i32) -> i32 {
    match section {
        GAME_SECTION_NEW => app.get_new_game_selected_index(),
        GAME_SECTION_TOP => app.get_top_game_selected_index(),
        GAME_SECTION_ALL => app.get_all_game_selected_index(),
        _ => 0,
    }
}

fn set_game_section_selected_index(app: &Demo, section: i32, index: i32) {
    match section {
        GAME_SECTION_NEW => app.set_new_game_selected_index(index),
        GAME_SECTION_TOP => app.set_top_game_selected_index(index),
        GAME_SECTION_ALL => app.set_all_game_selected_index(index),
        _ => {}
    }
}

fn first_available_game_section(app: &Demo) -> i32 {
    [GAME_SECTION_NEW, GAME_SECTION_TOP, GAME_SECTION_ALL]
        .into_iter()
        .find(|section| game_section_row_count(app, *section) > 0)
        .unwrap_or(GAME_SECTION_NEW)
}

fn normalize_games_selected_section(app: &Demo) -> i32 {
    let current = app.get_games_selected_section().clamp(GAME_SECTION_NEW, GAME_SECTION_ALL);
    let section = if game_section_row_count(app, current) > 0 {
        current
    } else {
        first_available_game_section(app)
    };
    app.set_games_selected_section(section);
    section
}

fn sync_selected_game_from_games_section(app: &Demo) {
    let section = normalize_games_selected_section(app);
    let row_count = game_section_row_count(app, section);
    if row_count == 0 {
        app.set_selected_game(0);
        return;
    }

    let selected_index = game_section_selected_index(app, section).clamp(0, row_count as i32 - 1);
    if selected_index != game_section_selected_index(app, section) {
        set_game_section_selected_index(app, section, selected_index);
    }

    let indices = match section {
        GAME_SECTION_NEW => app.get_new_game_indices(),
        GAME_SECTION_TOP => app.get_top_game_indices(),
        GAME_SECTION_ALL => app.get_all_game_indices(),
        _ => return,
    };

    if let Some(game_index) = indices.row_data(selected_index as usize) {
        app.invoke_select_game(game_index);
    }
}

fn move_games_section_vertical(app: &Demo, delta: i32) -> bool {
    let current = normalize_games_selected_section(app);
    let mut section = current + delta;
    while (GAME_SECTION_NEW..=GAME_SECTION_ALL).contains(&section) {
        if game_section_row_count(app, section) > 0 {
            app.set_games_selected_section(section);
            set_game_section_selected_index(app, section, 0);
            sync_selected_game_from_games_section(app);
            return true;
        }
        section += delta;
    }
    false
}

fn move_games_section_horizontal(app: &Demo, delta: i32) -> bool {
    let section = normalize_games_selected_section(app);
    let row_count = game_section_row_count(app, section) as i32;
    if row_count <= 0 {
        return false;
    }

    let current = game_section_selected_index(app, section).clamp(0, row_count - 1);
    let next = (current + delta).clamp(0, row_count - 1);
    if next == current {
        return false;
    }

    set_game_section_selected_index(app, section, next);
    sync_selected_game_from_games_section(app);
    true
}

fn refresh_controller_list(app: &Demo, selected_index: &Arc<Mutex<usize>>) {
    match load_store() {
        Ok(store) => {
            let mut selected = selected_index.lock().expect("selected_index lock poisoned");
            let controller_names = store
                .controllers
                .iter()
                .map(|controller| controller.name.clone().into())
                .collect::<Vec<SharedString>>();

            if *selected > store.controllers.len() {
                *selected = store.controllers.len();
            }

            if store.controllers.is_empty() {
                *selected = 0;
                app.set_controller_list_text("No controllers added yet.".into());
            } else {
                app.set_controller_list_text(
                    list_text_with_selection(&store, Some((*selected).min(store.controllers.len() - 1)))
                        .into(),
                );
            }

            app.set_controller_item_names(ModelRc::new(VecModel::from(controller_names)));
            app.set_controller_selected_index(*selected as i32);
            app.set_controller_manage_action(0);

            if app.get_controller_manager_status().is_empty() {
                app.set_controller_manager_status("".into());
            }
        }
        Err(error) => {
            log_launch(&format!("failed to load controller list: {error}"));
            app.set_controller_list_text("No controllers added yet.".into());
            app.set_controller_item_names(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            app.set_controller_selected_index(0);
            app.set_controller_manage_action(0);
            app.set_controller_manager_status(
                format!("Failed to load controllers: {error}").into(),
            );
        }
    }
}

fn refresh_system_settings(app: &Demo) {
    match load_or_create_system_settings() {
        Ok(settings) => {
            apply_system_settings_to_app(app, &settings);
            app.set_system_settings_status("".into());
        }
        Err(error) => {
            log_launch(&format!("failed to load system settings: {error}"));
            app.set_wifi_enabled(false);
            app.set_bluetooth_enabled(false);
            app.set_ethernet_status_title("Disconnected".into());
            app.set_ethernet_status_detail("No Ethernet link detected.".into());
            app.set_wifi_status_title("Disabled".into());
            app.set_wifi_status_detail("Enable Wi-Fi to scan and connect to a network.".into());
            app.set_system_settings_status(
                format!("Failed to load system settings: {error}").into(),
            );
        }
    }
}

fn poll_system_settings(app: &Demo) {
    match load_or_create_system_settings() {
        Ok(settings) => apply_system_settings_to_app(app, &settings),
        Err(error) => {
            log_launch(&format!("failed to poll system settings: {error}"));
        }
    }
}

#[derive(Default)]
struct WifiPasswordEditorState {
    ssid: String,
    password: String,
    selected_key: String,
    keyboard_mode: i32,
    show_password: bool,
}

#[derive(Default)]
struct BluetoothDebugState {
    lines: Vec<String>,
    offset: usize,
}

fn render_bluetooth_debug_text(state: &BluetoothDebugState) -> String {
    if state.lines.is_empty() {
        return "Bluetooth scan details will appear here.".to_string();
    }

    let max_offset = state.lines.len().saturating_sub(BLUETOOTH_DEBUG_VISIBLE_LINES);
    let offset = state.offset.min(max_offset);
    let end = (offset + BLUETOOTH_DEBUG_VISIBLE_LINES).min(state.lines.len());
    let mut visible = Vec::with_capacity(end.saturating_sub(offset) + 1);
    visible.push(format!(
        "Showing lines {}-{} of {}",
        offset + 1,
        end,
        state.lines.len()
    ));
    visible.extend(state.lines[offset..end].iter().cloned());
    visible.join("\n")
}

fn set_bluetooth_debug_report(
    app: &Demo,
    bluetooth_debug_state: &Arc<Mutex<BluetoothDebugState>>,
    report: String,
) {
    let rendered = {
        let mut guard = bluetooth_debug_state
            .lock()
            .expect("bluetooth debug state lock poisoned");
        guard.lines = report.lines().map(|line| line.to_string()).collect();
        guard.offset = 0;
        render_bluetooth_debug_text(&guard)
    };

    app.set_bluetooth_debug_text(rendered.into());
}

fn apply_system_settings_to_app(app: &Demo, settings: &SystemSettings) {
    app.set_wifi_enabled(settings.wifi_enabled);
    app.set_bluetooth_enabled(settings.bluetooth_enabled);

    let status = read_status(
        settings.wifi_enabled,
        settings.preferred_wifi_network.as_deref(),
    );
    let ethernet_title = if status.ethernet_active {
        "Connected".to_string()
    } else {
        "Disconnected".to_string()
    };
    let ethernet_detail = if status.ethernet_active {
        let mut detail = if !status.ethernet_ipv4_addresses.is_empty() {
            format!("Network cable is connected. IPv4: {}.", status.ethernet_ipv4_addresses.join(", "))
        } else {
            "Network cable is connected.".to_string()
        };
        if !status.ethernet_ipv6_addresses.is_empty() {
            detail.push_str(&format!(" IPv6: {}.", status.ethernet_ipv6_addresses.join(", ")));
        }
        detail
    } else {
        "No network cable is connected.".to_string()
    };

    let wifi_configured = status.saved_ssid.is_some();
    let wifi_connected = status.connected_ssid.is_some();
    let wifi_title = if status.ethernet_active {
        if wifi_connected {
            "Connected".to_string()
        } else if wifi_configured {
            "Configured".to_string()
        } else {
            "Configure".to_string()
        }
    } else if wifi_connected {
        "Connected".to_string()
    } else if wifi_configured {
        "Disconnected".to_string()
    } else {
        "Configure".to_string()
    };

    let wifi_detail = if status.ethernet_active {
        if wifi_connected {
            "Click to change network.".to_string()
        } else if wifi_configured {
            "Inactive due to connected network cable. WiFi will become active when network cable disconnects.".to_string()
        } else {
            "Search and select your WiFi network".to_string()
        }
    } else if wifi_connected {
        "Your wifi connection has been configured.".to_string()
    } else {
        "Search and select your WiFi network".to_string()
    };

    app.set_ethernet_status_title(ethernet_title.into());
    app.set_ethernet_status_detail(ethernet_detail.into());
    app.set_wifi_status_title(wifi_title.into());
    app.set_wifi_status_detail(wifi_detail.into());

    if !settings.wifi_enabled && app.get_system_setting() > 1 {
        app.set_system_setting(1);
    }
}

fn visible_wifi_network_window(networks: &[WifiNetwork], selected_index: usize) -> (usize, usize) {
    let selected_index = selected_index.min(networks.len() - 1);
    let mut start = selected_index.saturating_sub(WIFI_NETWORK_WINDOW_SIZE / 2);
    let end = (start + WIFI_NETWORK_WINDOW_SIZE).min(networks.len());
    if end - start < WIFI_NETWORK_WINDOW_SIZE {
        start = end.saturating_sub(WIFI_NETWORK_WINDOW_SIZE);
    }

    (start, selected_index)
}

fn sync_wifi_network_rows(app: &Demo, networks: &[WifiNetwork], selected_index: usize) {
    if networks.is_empty() {
        app.set_wifi_network_names(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
        app.set_wifi_network_secured(ModelRc::new(VecModel::from(Vec::<bool>::new())));
        app.set_wifi_network_states(ModelRc::new(VecModel::from(Vec::<i32>::new())));
        app.set_wifi_network_window_start(0);
        app.set_wifi_network_visible_selected_index(0);
        return;
    }

    let (start, selected_index) = visible_wifi_network_window(networks, selected_index);
    let end = (start + WIFI_NETWORK_WINDOW_SIZE).min(networks.len());
    let mut names = Vec::with_capacity(end - start);
    let mut secured = Vec::with_capacity(end - start);
    let mut states = Vec::with_capacity(end - start);

    for network in networks.iter().skip(start).take(end - start) {
        names.push(SharedString::from(network.ssid.clone()));
        secured.push(network.secured);
        states.push(if network.connected {
            WIFI_NETWORK_STATE_CONNECTED
        } else if network.known {
            WIFI_NETWORK_STATE_UNABLE_TO_CONNECT
        } else {
            WIFI_NETWORK_STATE_NONE
        });
    }

    app.set_wifi_network_names(ModelRc::new(VecModel::from(names)));
    app.set_wifi_network_secured(ModelRc::new(VecModel::from(secured)));
    app.set_wifi_network_states(ModelRc::new(VecModel::from(states)));
    app.set_wifi_network_window_start(start as i32);
    app.set_wifi_network_visible_selected_index((selected_index - start) as i32);
}

fn sync_wifi_network_selection(app: &Demo, networks: &[WifiNetwork], selected_index: usize) {
    if networks.is_empty() {
        app.set_wifi_network_selected_index(0);
        sync_wifi_network_rows(app, networks, 0);
        app.set_wifi_network_status("".into());
        return;
    }

    let selected_index = selected_index.min(networks.len() - 1);
    let selected = &networks[selected_index];
    app.set_wifi_network_selected_index(selected_index as i32);
    sync_wifi_network_rows(app, networks, selected_index);
    let status_message = if selected.known {
        format!("Press Enter to remove the saved Wi-Fi configuration for {}.", selected.ssid)
    } else if selected.secured {
        format!("Press Enter to enter the Wi-Fi password for {}.", selected.ssid)
    } else {
        format!("Press Enter to connect to {}.", selected.ssid)
    };
    app.set_wifi_network_status(status_message.into());
}

fn persist_wifi_selection(app: &Demo, ssid: &str) -> Result<SystemSettings, String> {
    let settings = update_system_settings(app, |settings| {
        settings.wifi_enabled = true;
        settings.preferred_wifi_network = Some(ssid.to_string());
    })?;

    apply_system_settings(&settings)?;
    apply_system_settings_to_app(app, &settings);
    Ok(settings)
}

fn current_system_setting_max_index(app: &Demo) -> i32 {
    if app.get_wifi_enabled() { 2 } else { 1 }
}

fn refresh_wifi_setup_screen(app: &Demo, wifi_networks: &Arc<Mutex<Vec<WifiNetwork>>>) {
    if app.get_screen() != 9 {
        return;
    }

    let mut guard = wifi_networks.lock().expect("wifi networks lock poisoned");
    if guard.is_empty() {
        app.set_wifi_network_selected_index(0);
        sync_wifi_network_rows(app, &guard, 0);
        if app.get_wifi_enabled() {
            app.set_wifi_network_placeholder_text("Searching for networks...".into());
            app.set_wifi_network_status("".into());
        } else {
            app.set_wifi_network_placeholder_text(
                "Wi-Fi is disabled. Enable it in System Settings to scan for networks."
                    .into(),
            );
            app.set_wifi_network_status("".into());
        }
        return;
    }

    refresh_network_status(&mut guard);
    app.set_wifi_network_placeholder_text("".into());
    let selected_index = app.get_wifi_network_selected_index().max(0) as usize;
    sync_wifi_network_selection(app, &guard, selected_index);
}

fn wifi_password_rows_for_mode(mode: i32) -> &'static [&'static [&'static str]] {
    match mode {
        1 => &WIFI_PASSWORD_CAPITAL_ROWS,
        2 => &WIFI_PASSWORD_SPECIFIC_ROWS,
        _ => &WIFI_PASSWORD_REGULAR_ROWS,
    }
}

fn wifi_password_action_for_mode(mode: i32) -> i32 {
    match mode {
        1 => WIFI_PASSWORD_ACTION_MODE_UPPER,
        2 => WIFI_PASSWORD_ACTION_MODE_SYMBOLS,
        _ => WIFI_PASSWORD_ACTION_MODE_LOWER,
    }
}

fn wifi_password_key_label(key: &str) -> &str {
    key
}

fn is_allowed_wifi_password_key(text: &str) -> bool {
    if text == " " {
        return true;
    }

    [
        &WIFI_PASSWORD_REGULAR_ROWS,
        &WIFI_PASSWORD_CAPITAL_ROWS,
        &WIFI_PASSWORD_SPECIFIC_ROWS,
    ]
    .into_iter()
    .flat_map(|rows| rows.iter())
    .flat_map(|row| row.iter())
    .any(|key| *key == text && *key != "Space" && *key != "Delete")
}

fn wifi_password_key_position_in_rows(
    rows: &[&[&str]],
    selected_key: &str,
) -> Option<(usize, usize)> {
    for (row_index, row) in rows.iter().enumerate() {
        for (column_index, key) in row.iter().enumerate() {
            if *key == selected_key || key.eq_ignore_ascii_case(selected_key) {
                return Some((row_index, column_index));
            }
        }
    }

    None
}

fn wifi_password_selected_key_position(state: &WifiPasswordEditorState) -> (usize, usize) {
    let rows = wifi_password_rows_for_mode(state.keyboard_mode);
    wifi_password_key_position_in_rows(rows, &state.selected_key).unwrap_or((0, 0))
}

fn wifi_password_select_position(
    state: &mut WifiPasswordEditorState,
    row_index: usize,
    column_index: usize,
) {
    let rows = wifi_password_rows_for_mode(state.keyboard_mode);
    let row_index = row_index.min(rows.len().saturating_sub(1));
    let row = rows[row_index];
    let column_index = column_index.min(row.len().saturating_sub(1));
    state.selected_key = row[column_index].to_string();
}

fn wifi_password_set_keyboard_mode(state: &mut WifiPasswordEditorState, keyboard_mode: i32) {
    let (row_index, column_index) = wifi_password_selected_key_position(state);
    state.keyboard_mode = keyboard_mode.clamp(0, 2);
    wifi_password_select_position(state, row_index, column_index);
}

fn wifi_password_visual_x2(row_len: usize, column_index: usize) -> i32 {
    (column_index as i32 * 2) - row_len as i32
}

fn wifi_password_select_nearest_vertical_key(
    state: &mut WifiPasswordEditorState,
    target_row_index: usize,
    current_x2: i32,
    exclude_edge_keys: bool,
) {
    let rows = wifi_password_rows_for_mode(state.keyboard_mode);
    let target_row = rows[target_row_index];
    let mut start_index = 0;
    let mut end_index = target_row.len();

    if exclude_edge_keys && target_row.len() > 2 {
        start_index = 1;
        end_index = target_row.len() - 1;
    }

    if start_index >= end_index {
        start_index = 0;
        end_index = target_row.len();
    }

    let next_column_index = (start_index..end_index)
        .min_by_key(|column_index| {
            (
                (wifi_password_visual_x2(target_row.len(), *column_index) - current_x2).abs(),
                *column_index,
            )
        })
        .unwrap_or(0);

    state.selected_key = target_row[next_column_index].to_string();
}

fn wifi_password_display_text(state: &WifiPasswordEditorState) -> String {
    if state.show_password {
        state.password.clone()
    } else {
        "*".repeat(state.password.chars().count())
    }
}

fn is_allowed_wifi_password_text(text: &str) -> bool {
    text.chars().count() == 1 && is_allowed_wifi_password_key(text)
}

fn is_wifi_password_controller_cancel_token(token: &str) -> bool {
    matches!(
        token,
        "SDL_CONTROLLER_BUTTON_B"
            | "SDL_CONTROLLER_BUTTON_BACK"
            | "SDL_JOYSTICK_BUTTON_1"
    )
}

fn append_wifi_password_text(
    app: &Demo,
    editor_state: &Arc<Mutex<WifiPasswordEditorState>>,
    text: &str,
    status: &str,
) {
    if !is_allowed_wifi_password_text(text) {
        return;
    }

    let ch = text.chars().next().expect("validated single character");

    let mut guard = editor_state.lock().expect("wifi password state lock poisoned");
    guard.password.push(ch);
    sync_wifi_password_editor(app, &guard, status);
}

fn sync_wifi_password_editor(app: &Demo, state: &WifiPasswordEditorState, status: &str) {
    app.set_wifi_password_network_name(state.ssid.clone().into());
    app.set_wifi_password_input_text(wifi_password_display_text(state).into());
    app.set_wifi_password_keyboard_mode(state.keyboard_mode);
    app.set_wifi_password_key_selected(wifi_password_key_label(&state.selected_key).into());
    app.set_wifi_password_status(status.into());
    app.set_wifi_password_visibility_label(
        if state.show_password { "HIDE" } else { "SHOW" }.into(),
    );
}

fn move_wifi_password_key_selection(
    app: &Demo,
    editor_state: &Arc<Mutex<WifiPasswordEditorState>>,
    delta_row: isize,
    delta_col: isize,
) {
    let mut guard = editor_state.lock().expect("wifi password state lock poisoned");
    let rows = wifi_password_rows_for_mode(guard.keyboard_mode);
    let (current_row, current_col) = wifi_password_selected_key_position(&guard);

    if delta_col != 0 {
        let row = rows[current_row];
        let next_col = if delta_col < 0 {
            if current_col == 0 {
                row.len() - 1
            } else {
                current_col - 1
            }
        } else if current_col + 1 >= row.len() {
            0
        } else {
            current_col + 1
        };
        guard.selected_key = row[next_col].to_string();
        sync_wifi_password_editor(app, &guard, app.get_wifi_password_status().as_str());
        return;
    }

    if delta_row == 0 {
        sync_wifi_password_editor(app, &guard, app.get_wifi_password_status().as_str());
        return;
    }

    if delta_row > 0 {
        if current_row + 1 >= rows.len() {
            app.set_wifi_password_action(wifi_password_action_for_mode(guard.keyboard_mode));
            sync_wifi_password_editor(app, &guard, app.get_wifi_password_status().as_str());
            return;
        }

        let current_x2 = wifi_password_visual_x2(rows[current_row].len(), current_col);
        wifi_password_select_nearest_vertical_key(&mut guard, current_row + 1, current_x2, false);
    } else if current_row == 0 {
        app.set_wifi_password_action(WIFI_PASSWORD_ACTION_VISIBILITY);
        sync_wifi_password_editor(app, &guard, app.get_wifi_password_status().as_str());
        return;
    } else {
        let current_x2 = wifi_password_visual_x2(rows[current_row].len(), current_col);
        wifi_password_select_nearest_vertical_key(&mut guard, current_row - 1, current_x2, current_row - 1 == 0);
    }

    sync_wifi_password_editor(app, &guard, app.get_wifi_password_status().as_str());
}

fn clear_wifi_password_editor(
    app: &Demo,
    editor_state: &Arc<Mutex<WifiPasswordEditorState>>,
    status: &str,
) {
    let mut guard = editor_state.lock().expect("wifi password state lock poisoned");
    guard.password.clear();
    guard.ssid.clear();
    guard.selected_key = WIFI_PASSWORD_REGULAR_ROW_0[0].to_string();
    guard.keyboard_mode = 0;
    guard.show_password = false;
    app.set_wifi_password_action(WIFI_PASSWORD_ACTION_KEYS);
    sync_wifi_password_editor(app, &guard, status);
}

fn open_wifi_password_editor(
    app: &Demo,
    editor_state: &Arc<Mutex<WifiPasswordEditorState>>,
    ssid: &str,
    status: &str,
) {
    let mut guard = editor_state.lock().expect("wifi password state lock poisoned");
    guard.ssid = ssid.to_string();
    guard.password.clear();
    guard.selected_key = WIFI_PASSWORD_REGULAR_ROW_0[0].to_string();
    guard.keyboard_mode = 0;
    guard.show_password = false;
    app.set_wifi_password_action(WIFI_PASSWORD_ACTION_KEYS);
    sync_wifi_password_editor(app, &guard, status);
    app.set_screen(10);
    app.set_focus_region(1);
}

fn handle_console_input_event(
    app: &Demo,
    event: &ConsoleInputEvent,
    setup_state: &Arc<Mutex<Option<ControllerSetupState>>>,
    selected_index: &Arc<Mutex<usize>>,
    wifi_password_state: &Arc<Mutex<WifiPasswordEditorState>>,
) {
    match &event.phase {
        ConsoleInputPhase::Connected => {
            if let ConsoleInputSource::Controller {
                controller_name, ..
            } = &event.source
            {
                app.set_controller_manager_status(
                    format!("Controller connected: {controller_name}").into(),
                );
            }
            return;
        }
        ConsoleInputPhase::Disconnected => {
            if let ConsoleInputSource::Controller {
                controller_name, ..
            } = &event.source
            {
                app.set_controller_manager_status(
                    format!("Controller disconnected: {controller_name}").into(),
                );
            }
            return;
        }
        ConsoleInputPhase::Released => return,
        ConsoleInputPhase::Pressed | ConsoleInputPhase::Repeat => {}
    }

    if app.get_screen() == 6 && matches!(event.phase, ConsoleInputPhase::Pressed) {
        let update = {
            let mut guard = setup_state.lock().expect("setup_state lock poisoned");
            match guard.as_mut() {
                Some(state) if state.is_mapping_controls() => match &event.source {
                    ConsoleInputSource::Controller { device_id, .. } => {
                        if state
                            .selected_device_id()
                            .as_deref()
                            .is_some_and(|expected| expected != device_id)
                        {
                            None
                        } else {
                            let finished = state.register_mapping_press(&event.token);
                            let hits = state.current_press_count();
                            let progress = state.current_progress_percent();
                            let should_play_rotation_animation =
                                state.should_play_rotation_animation();
                            let display = if hits == 0 {
                                format!("Mapped {} (10/10)", event.token)
                            } else {
                                format!("{} ({hits}/10)", event.token)
                            };
                            Some((finished, progress, display, should_play_rotation_animation))
                        }
                    }
                    ConsoleInputSource::Keyboard { .. } => None,
                },
                _ => None,
            }
        };

        if let Some((finished, progress, display, should_play_rotation_animation)) = update {
            refresh_controller_setup_view(app, setup_state);
            app.set_controller_config_progress_percent(progress);
            app.set_controller_config_last_input(display.into());
            if should_play_rotation_animation {
                play_dualsense_rotation_animation(app, setup_state);
            }
            if finished {
                log_launch("controller setup: mapping complete, scheduling save");
                let app_weak = app.as_weak();
                let setup_state = Arc::clone(setup_state);
                let selected_index = Arc::clone(selected_index);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    finalize_controller_setup(&app, &setup_state, &selected_index, true);
                });
            }
            return;
        }
    }

    if app.get_screen() == 10
        && matches!(event.source, ConsoleInputSource::Keyboard { .. })
        && event.action.is_some()
        && !matches!(event.action, Some(ConsoleAction::Up | ConsoleAction::Down | ConsoleAction::Left | ConsoleAction::Right | ConsoleAction::Cancel))
    {
        return;
    }

    if app.get_screen() == 10
        && matches!(event.phase, ConsoleInputPhase::Pressed)
        && matches!(event.source, ConsoleInputSource::Controller { .. })
        && event.action.is_none()
        && is_wifi_password_controller_cancel_token(&event.token)
    {
        handle_console_action(
            app,
            ConsoleAction::Cancel,
            setup_state,
            wifi_password_state,
        );
        return;
    }

    if let Some(action) = event.action {
        handle_console_action(
            app,
            action,
            setup_state,
            wifi_password_state,
        );
    }
}

fn handle_console_action(
    app: &Demo,
    action: ConsoleAction,
    setup_state: &Arc<Mutex<Option<ControllerSetupState>>>,
    wifi_password_state: &Arc<Mutex<WifiPasswordEditorState>>,
) {
    match action {
        ConsoleAction::Up => {
            if app.get_screen() == 3 && app.get_screenshot_popup_open() {
                return;
            }

            if app.get_screen() == 3 && app.get_focus_region() == 1 {
                if app.get_selected_detail_control() == 3 {
                    app.set_selected_detail_control(1);
                } else if app.get_selected_detail_control() == 1
                    || app.get_selected_detail_control() == 2
                {
                    app.set_nav_selection(1);
                    app.set_focus_region(0);
                }
                return;
            }

            if app.get_screen() == 12 && app.get_focus_region() == 1 {
                if app.get_network_options_selected_index() > 0 {
                    app.set_network_options_selected_index(0);
                }
                return;
            }

            if app.get_screen() == 1 && app.get_focus_region() == 1 {
                if !move_games_section_vertical(app, -1) {
                    app.set_focus_region(0);
                }
                return;
            }

            if app.get_screen() == 0 && app.get_focus_region() == 1 {
                if !move_home_section_vertical(app, -1) {
                    app.set_focus_region(0);
                }
                return;
            }

            if app.get_focus_region() == 0 && app.get_nav_selection() > 0 {
                app.set_nav_selection(app.get_nav_selection() - 1);
                app.set_screen(app.get_nav_selection());
                return;
            }

            if app.get_screen() == 8 {
                if !app.get_bluetooth_pairing_intro()
                    && !app.get_bluetooth_scan_running()
                    && app.get_bluetooth_device_action() == BLUETOOTH_ACTION_DEVICE_LIST
                    && app.get_bluetooth_device_selected_index() > 0
                {
                    app.invoke_select_bluetooth_device(app.get_bluetooth_device_selected_index() - 1);
                } else {
                    app.set_bluetooth_device_action(BLUETOOTH_ACTION_PRIMARY_BUTTON);
                }
                return;
            }

            if app.get_screen() == 9 {
                if app.get_wifi_network_selected_index() > 0 {
                    app.invoke_select_wifi_network(app.get_wifi_network_selected_index() - 1);
                }
                return;
            }

            if app.get_screen() == 10 {
                if app.get_wifi_password_action() == WIFI_PASSWORD_ACTION_KEYS {
                    move_wifi_password_key_selection(app, wifi_password_state, -1, 0);
                } else if app.get_wifi_password_action() == WIFI_PASSWORD_ACTION_CONNECT {
                    let mode_action = {
                        let guard = wifi_password_state
                            .lock()
                            .expect("wifi password state lock poisoned");
                        wifi_password_action_for_mode(guard.keyboard_mode)
                    };
                    app.set_wifi_password_action(mode_action);
                } else {
                    app.set_wifi_password_action(WIFI_PASSWORD_ACTION_KEYS);
                }
                return;
            }

            if app.get_screen() == 2
                && app.get_focus_region() == 1
                && app.get_selected_setting() > 0
            {
                app.set_selected_setting(app.get_selected_setting() - 1);
                return;
            }

            if app.get_screen() == 7 && app.get_focus_region() == 1 && app.get_system_setting() > 0
            {
                app.set_system_setting(app.get_system_setting() - 1);
                return;
            }

            if app.get_screen() == 6 && app.get_controller_device_selection() {
                app.invoke_controller_device_select_prev();
                return;
            }

            if app.get_screen() == 4 && app.get_focus_region() == 1 {
                if app.get_controller_manage_action() == 1 {
                    app.set_controller_manage_action(0);
                } else if app.get_controller_selected_index() > 0 {
                    app.invoke_controller_select_prev();
                }
                return;
            }

            if app.get_focus_region() == 1 {
                app.set_focus_region(0);
            }
        }
        ConsoleAction::Down => {
            if app.get_screen() == 3 && app.get_screenshot_popup_open() {
                return;
            }

            if app.get_screen() == 3 && app.get_focus_region() == 1 {
                if app.get_selected_detail_control() == 0 {
                    app.set_selected_detail_control(1);
                } else if app.get_selected_detail_control() == 1
                    || app.get_selected_detail_control() == 2
                {
                    app.set_selected_detail_control(3);
                    app.set_selected_screenshot_index(0);
                }
                return;
            }

            if app.get_screen() == 4 && app.get_focus_region() == 1 {
                if app.get_controller_manage_action() == 1 {
                    app.set_controller_manage_action(0);
                }
                app.invoke_controller_select_next();
                return;
            }

            if app.get_screen() == 12 && app.get_focus_region() == 1 {
                if app.get_network_options_selected_index() < 1 {
                    app.set_network_options_selected_index(1);
                }
                return;
            }

            if app.get_screen() == 1 && app.get_focus_region() == 0 {
                app.set_games_selected_section(first_available_game_section(app));
                sync_selected_game_from_games_section(app);
                app.set_focus_region(1);
                return;
            }

            if app.get_screen() == 0 && app.get_focus_region() == 0 {
                if let Some(section) = first_available_home_section(app) {
                    app.set_home_selected_section(section);
                    sync_selected_game_from_home_section(app);
                    app.set_focus_region(1);
                }
                return;
            }

            if app.get_screen() == 3 && app.get_focus_region() == 0 {
                if app.get_nav_selection() == 1 {
                    app.set_selected_detail_control(1);
                }
                app.set_focus_region(1);
                return;
            }

            if app.get_screen() == 0 && app.get_focus_region() == 1 {
                let _ = move_home_section_vertical(app, 1);
                return;
            }

            if app.get_screen() == 1 && app.get_focus_region() == 1 {
                let _ = move_games_section_vertical(app, 1);
                return;
            }

            if app.get_screen() == 9 {
                let next_index = app.get_wifi_network_selected_index() + 1;
                app.invoke_select_wifi_network(next_index);
                return;
            }

            if app.get_screen() == 10 {
                if app.get_wifi_password_action() == WIFI_PASSWORD_ACTION_KEYS {
                    move_wifi_password_key_selection(app, wifi_password_state, 1, 0);
                } else if matches!(
                    app.get_wifi_password_action(),
                    WIFI_PASSWORD_ACTION_MODE_UPPER
                        | WIFI_PASSWORD_ACTION_MODE_LOWER
                        | WIFI_PASSWORD_ACTION_MODE_SYMBOLS
                ) {
                    app.set_wifi_password_action(WIFI_PASSWORD_ACTION_CONNECT);
                } else if app.get_wifi_password_action() == WIFI_PASSWORD_ACTION_VISIBILITY {
                    app.set_wifi_password_action(WIFI_PASSWORD_ACTION_KEYS);
                }
                return;
            }

            if app.get_focus_region() == 0 {
                app.set_focus_region(1);
                return;
            }

            if app.get_screen() == 2 && app.get_selected_setting() < 4 {
                app.set_selected_setting(app.get_selected_setting() + 1);
                return;
            }

            if app.get_screen() == 7
                && app.get_focus_region() == 1
                && app.get_system_setting() < current_system_setting_max_index(app)
            {
                app.set_system_setting(app.get_system_setting() + 1);
                return;
            }

            if app.get_screen() == 6 && app.get_controller_device_selection() {
                app.invoke_controller_device_select_next();
                return;
            }

            if app.get_screen() == 4 && app.get_focus_region() == 1 {
                app.invoke_controller_select_next();
            }
        }
        ConsoleAction::Left => {
            if app.get_screen() == 3
                && app.get_focus_region() == 1
                && !app.get_game_launching()
            {
                if app.get_selected_detail_control() == 2 {
                    app.set_selected_detail_control(1);
                } else if app.get_selected_detail_control() == 3
                    && app.get_selected_screenshot_index() > 0
                {
                    app.set_selected_screenshot_index(app.get_selected_screenshot_index() - 1);
                }
                return;
            }

            if app.get_screen() == 3
                && app.get_focus_region() == 1
                && !app.get_game_launching()
            {
                if app.get_selected_detail_control() == 0 {
                    return;
                }

                app.set_selected_detail_control(0);
                return;
            }

            if app.get_screen() == 12 && app.get_focus_region() == 1 {
                if app.get_network_options_selected_index() > 0 {
                    app.set_network_options_selected_index(0);
                }
                return;
            }

            if app.get_focus_region() == 0 && app.get_nav_selection() > 0 {
                app.set_nav_selection(app.get_nav_selection() - 1);
                app.set_screen(app.get_nav_selection());
                return;
            }

            if app.get_screen() == 0 && app.get_focus_region() == 1 {
                let _ = move_home_section_horizontal(app, -1);
                return;
            }

            if app.get_screen() == 1 && app.get_focus_region() == 1 {
                let _ = move_games_section_horizontal(app, -1);
                return;
            }

            if app.get_screen() == 8 {
                if !app.get_bluetooth_pairing_intro()
                    && !app.get_bluetooth_scan_running()
                    && app.get_bluetooth_device_action() == BLUETOOTH_ACTION_DEVICE_LIST
                    && app.get_bluetooth_device_selected_index() > 0
                {
                    app.invoke_select_bluetooth_device(
                        app.get_bluetooth_device_selected_index() - 1,
                    );
                } else {
                    app.set_bluetooth_device_action(BLUETOOTH_ACTION_PRIMARY_BUTTON);
                }
                return;
            }

            if app.get_screen() == 9 {
                return;
            }

            if app.get_screen() == 10 {
                if app.get_wifi_password_action() == WIFI_PASSWORD_ACTION_KEYS {
                    move_wifi_password_key_selection(app, wifi_password_state, 0, -1);
                } else if matches!(
                    app.get_wifi_password_action(),
                    WIFI_PASSWORD_ACTION_MODE_LOWER
                        | WIFI_PASSWORD_ACTION_MODE_SYMBOLS
                ) {
                    app.set_wifi_password_action(app.get_wifi_password_action() - 1);
                }
                return;
            }

            if app.get_screen() == 7 && app.get_focus_region() == 1 && app.get_system_setting() > 0
            {
                app.set_system_setting(app.get_system_setting() - 1);
                return;
            }

            if app.get_screen() == 4 && app.get_focus_region() == 1 {
                app.set_controller_manage_action(0);
                return;
            }

            if app.get_screen() == 5 && app.get_controller_type_selection() > 0 {
                app.set_controller_type_selection(app.get_controller_type_selection() - 1);
                return;
            }

            if app.get_screen() == 6 && app.get_controller_config_complete() {
                app.set_controller_final_selection(0);
            }
        }
        ConsoleAction::Right => {
            if app.get_screen() == 3 && app.get_screenshot_popup_open() {
                return;
            }

            if app.get_screen() == 3
                && app.get_focus_region() == 1
                && !app.get_game_launching()
            {
                match app.get_selected_detail_control() {
                    1 => {
                        app.set_selected_detail_control(2);
                    }
                    2 => {
                        app.set_selected_detail_control(3);
                        app.set_selected_screenshot_index(0);
                    }
                    3 => {
                        let screenshot_count = app.get_game_screenshots().row_count() as i32;
                        if app.get_selected_screenshot_index() + 1 < screenshot_count {
                            app.set_selected_screenshot_index(
                                app.get_selected_screenshot_index() + 1,
                            );
                        }
                    }
                    _ => {}
                }
                return;
            }

            if app.get_screen() == 12 && app.get_focus_region() == 1 {
                if app.get_network_options_selected_index() < 1 {
                    app.set_network_options_selected_index(1);
                }
                return;
            }

            if app.get_focus_region() == 0 && app.get_nav_selection() < 2 {
                app.set_nav_selection(app.get_nav_selection() + 1);
                app.set_screen(app.get_nav_selection());
                return;
            }

            if app.get_screen() == 0 && app.get_focus_region() == 1 {
                let _ = move_home_section_horizontal(app, 1);
                return;
            }

            if app.get_screen() == 1
                && app.get_focus_region() == 1
            {
                let _ = move_games_section_horizontal(app, 1);
                return;
            }

            if app.get_screen() == 8 {
                if !app.get_bluetooth_pairing_intro()
                    && !app.get_bluetooth_scan_running()
                    && app.get_bluetooth_device_action() == BLUETOOTH_ACTION_DEVICE_LIST
                {
                    app.invoke_select_bluetooth_device(
                        app.get_bluetooth_device_selected_index() + 1,
                    );
                } else if !app.get_bluetooth_pairing_intro()
                    && !app.get_bluetooth_scan_running()
                    && app.get_bluetooth_device_names().row_count() > 0
                {
                    app.set_bluetooth_device_action(BLUETOOTH_ACTION_DEVICE_LIST);
                } else {
                    app.set_bluetooth_device_action(BLUETOOTH_ACTION_PRIMARY_BUTTON);
                }
                return;
            }

            if app.get_screen() == 9 {
                return;
            }

            if app.get_screen() == 10 {
                if app.get_wifi_password_action() == WIFI_PASSWORD_ACTION_KEYS {
                    move_wifi_password_key_selection(app, wifi_password_state, 0, 1);
                } else if matches!(
                    app.get_wifi_password_action(),
                    WIFI_PASSWORD_ACTION_MODE_UPPER
                        | WIFI_PASSWORD_ACTION_MODE_LOWER
                ) {
                    app.set_wifi_password_action(app.get_wifi_password_action() + 1);
                }
                return;
            }

            if app.get_screen() == 7
                && app.get_focus_region() == 1
                && app.get_system_setting() < current_system_setting_max_index(app)
            {
                app.set_system_setting(app.get_system_setting() + 1);
                return;
            }

            if app.get_screen() == 4 && app.get_focus_region() == 1 {
                if app.get_controller_selected_index() < app.get_controller_item_names().row_count() as i32 {
                    app.set_controller_manage_action(1);
                }
                return;
            }

            if app.get_screen() == 5 && app.get_controller_type_selection() < 2 {
                app.set_controller_type_selection(app.get_controller_type_selection() + 1);
                return;
            }

            if app.get_screen() == 6 && app.get_controller_config_complete() {
                app.set_controller_final_selection(1);
            }
        }
        ConsoleAction::Accept | ConsoleAction::Start => {
            if app.get_screen() == 3 && app.get_screenshot_popup_open() {
                app.set_screenshot_popup_open(false);
                return;
            }

            if app.get_focus_region() == 0 {
                app.set_screen(app.get_nav_selection());
                app.set_focus_region(1);
                return;
            }

            match app.get_screen() {
                0 => {
                    if app.get_focus_region() == 1 {
                        sync_selected_game_from_home_section(app);
                        app.set_screen(3);
                        app.set_selected_detail_control(1);
                    }
                }
                1 => {
                    if app.get_game_count() > 0 {
                        sync_selected_game_from_games_section(app);
                        app.set_screen(3);
                        app.set_selected_detail_control(1);
                    }
                }
                3 => {
                    if !app.get_game_launching() && app.get_selected_detail_control() == 0 {
                        app.set_screen(1);
                        app.set_launch_status("".into());
                    } else if !app.get_game_launching() && app.get_selected_detail_control() == 1 {
                        app.invoke_launch_game(app.get_selected_game());
                    } else if !app.get_game_launching() && app.get_selected_detail_control() == 2 {
                        app.invoke_toggle_game_favorite();
                    } else if app.get_selected_detail_control() == 3 {
                        let screenshot_count = app.get_game_screenshots().row_count() as i32;
                        if screenshot_count > 0 {
                            let selected_index = app
                                .get_selected_screenshot_index()
                                .max(0)
                                .min(screenshot_count - 1);
                            app.set_selected_screenshot_index(selected_index);
                            app.set_screenshot_popup_open(true);
                        }
                    }
                }
                2 => {
                    if app.get_selected_setting() == 0 {
                        app.set_screen(7);
                        app.set_focus_region(1);
                        app.set_selected_setting(0);
                        app.set_system_settings_status("".into());
                    } else if app.get_selected_setting() == 1 {
                        app.set_screen(12);
                        app.set_focus_region(1);
                        app.set_network_options_selected_index(1);
                    } else if app.get_selected_setting() == 2 {
                        app.set_screen(4);
                        app.set_controller_manager_status("".into());
                        app.invoke_refresh_controllers();
                    } else if app.get_selected_setting() == 3 {
                        app.set_screen(11);
                        app.set_focus_region(1);
                    } else if app.get_selected_setting() == 4 {
                        app.invoke_restart_system();
                    }
                }
                7 => {
                    if app.get_system_setting() == 0 {
                        app.invoke_toggle_wifi();
                    } else if app.get_system_setting() == 1 {
                        if app.get_wifi_enabled() {
                            app.invoke_open_wifi_setup();
                        } else {
                            app.invoke_toggle_bluetooth();
                        }
                    } else if app.get_system_setting() == 2 {
                        app.invoke_toggle_bluetooth();
                    }
                }
                12 => {
                    if app.get_network_options_selected_index() == 1 {
                        app.invoke_open_wifi_setup();
                    } else {
                        app.set_screen(2);
                        app.set_focus_region(1);
                    }
                }
                9 => app.invoke_activate_wifi_network(),
                10 => match app.get_wifi_password_action() {
                    0 => app.invoke_wifi_password_append_selected(),
                    1 => app.invoke_wifi_password_toggle_visibility(),
                    2 => app.invoke_wifi_password_select_mode(1),
                    3 => app.invoke_wifi_password_select_mode(0),
                    4 => app.invoke_wifi_password_select_mode(2),
                    5 => app.invoke_wifi_password_connect(),
                    _ => {}
                },
                4 => {
                    if app.get_controller_selected_index()
                        == app.get_controller_item_names().row_count() as i32
                    {
                        app.set_screen(5);
                        app.set_controller_type_selection(0);
                        app.set_controller_manager_status("".into());
                    } else if app.get_controller_manage_action() == 1 {
                        app.invoke_remove_selected_controller();
                    }
                }
                5 => {
                    if app.get_controller_type_selection() == 2 {
                        if !app.get_bluetooth_enabled() {
                            app.set_add_controller_status(
                                "Bluetooth is disabled. Enable it in System first.".into(),
                            );
                        } else {
                            app.set_add_controller_status("".into());
                            app.set_bluetooth_pairing_status(
                                "Press Continue when the controller starts blinking.".into(),
                            );
                            app.set_bluetooth_device_names(ModelRc::new(VecModel::from(vec![])));
                            app.set_bluetooth_device_list_text("".into());
                            app.set_bluetooth_debug_text("Bluetooth scan details will appear here.".into());
                            app.set_bluetooth_device_selected_index(0);
                            app.set_bluetooth_device_selected_paired(false);
                            app.set_bluetooth_device_action(BLUETOOTH_ACTION_PRIMARY_BUTTON);
                            app.set_bluetooth_pairing_intro(true);
                            app.set_bluetooth_scan_running(false);
                            app.set_bluetooth_link_light_off_visible(false);
                            app.set_focus_region(1);
                            app.set_screen(8);
                            start_bluetooth_pairing_light_blink(app.as_weak());
                        }
                    } else {
                        app.set_screen(6);
                        app.invoke_start_controller_setup(app.get_controller_type_selection());
                    }
                }
                8 => {
                    if app.get_bluetooth_pairing_intro() || app.get_bluetooth_device_names().row_count() == 0 {
                        app.invoke_scan_bluetooth_devices();
                    } else {
                        app.invoke_pair_or_continue_bluetooth_device();
                    }
                }
                6 => {
                    if app.get_controller_device_selection() {
                        app.invoke_activate_controller_device_selection();
                    } else if app.get_controller_config_complete() {
                        app.invoke_activate_controller_final_selection();
                    }
                }
                _ => {}
            }
        }
        ConsoleAction::Cancel => {
            if app.get_screen() == 3 && app.get_screenshot_popup_open() {
                app.set_screenshot_popup_open(false);
                return;
            } else if app.get_screen() == 10 {
                clear_wifi_password_editor(
                    app,
                    wifi_password_state,
                    "Wi-Fi password cleared.",
                );
                app.invoke_cancel_wifi_setup();
                return;
            }

            if app.get_screen() == 3 && !app.get_game_launching() {
                app.set_screen(1);
                app.set_focus_region(1);
                app.set_launch_status("".into());
            } else if app.get_screen() == 4 {
                app.set_screen(2);
                app.set_focus_region(1);
                app.set_selected_setting(1);
            } else if app.get_screen() == 5 {
                app.set_screen(4);
                app.set_focus_region(1);
            } else if app.get_screen() == 8 {
                app.set_focus_region(1);
                app.invoke_cancel_bluetooth_pairing();
            } else if app.get_screen() == 11 {
                app.set_screen(2);
                app.set_focus_region(1);
                app.set_selected_setting(3);
            } else if app.get_screen() == 12 {
                app.set_screen(2);
                app.set_focus_region(1);
                app.set_selected_setting(1);
            } else if app.get_screen() == 9 {
                app.invoke_cancel_wifi_setup();
            } else if app.get_screen() == 10 {
                app.invoke_wifi_password_cancel();
            } else if app.get_screen() == 6 {
                app.invoke_cancel_controller_setup();
            } else if app.get_screen() == 7 {
                app.set_screen(2);
                app.set_focus_region(1);
                app.set_selected_setting(0);
                app.set_system_settings_status("".into());
            } else if app.get_focus_region() == 1 && app.get_screen() != 9 && app.get_screen() != 10 {
                app.set_focus_region(0);
            }
        }
        ConsoleAction::Select => {}
    }

    let _ = setup_state;
}

fn format_bluetooth_devices(devices: &[BluetoothDeviceInfo], selected_index: usize) -> String {
    if devices.is_empty() {
        return "No DualSense device found yet. Press Scan / Refresh to search again.".to_string();
    }

    devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            let marker = if index == selected_index { ">" } else { " " };
            let state = if device.paired { "PAIRED" } else { "NEW" };
            format!(
                "{marker} [{state}] Device {} {}",
                device.address, device.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn bluetooth_device_row_labels(devices: &[BluetoothDeviceInfo]) -> Vec<SharedString> {
    devices
        .iter()
        .map(|device| SharedString::from(device.address.clone()))
        .collect()
}

fn format_bluetooth_scan_debug(report: &BluetoothScanReport, selected_index: usize) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "bluetoothctl raw output ({} bytes):",
        report.raw_output.len()
    ));
    lines.extend(report.raw_output.lines().map(|line| format!("  {line}")));

    lines.push(String::new());
    lines.push(format!(
        "all bluetooth devices ({}):",
        report.all_devices.len()
    ));

    if report.all_devices.is_empty() {
        lines.push("  (none found)".to_string());
    } else {
        for (index, device) in report.all_devices.iter().enumerate() {
            let marker = if index == selected_index { ">" } else { " " };
            let state = match (device.paired, device.trusted, device.connected) {
                (true, true, true) => "PAIRED TRUSTED CONNECTED",
                (true, true, false) => "PAIRED TRUSTED",
                (true, false, false) => "PAIRED",
                (false, true, false) => "TRUSTED",
                (false, false, true) => "CONNECTED",
                _ => "NEW",
            };
            lines.push(format!(
                "{marker} [{state}] Device {} {}",
                device.address, device.name
            ));
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "DualSense devices ({}):",
        report.dualsense_devices.len()
    ));

    if report.dualsense_devices.is_empty() {
        lines.push("  No DualSense device found.".to_string());
    } else {
        for (index, device) in report.dualsense_devices.iter().enumerate() {
            let marker = if index == selected_index { ">" } else { " " };
            let state = if device.paired { "PAIRED" } else { "NEW" };
            lines.push(format!(
                "{marker} [{state}] Device {} {}",
                device.address, device.name
            ));
        }
    }

    lines.join("\n")
}

fn sync_bluetooth_selection(app: &Demo, devices: &[BluetoothDeviceInfo], selected_index: usize) {
    app.set_bluetooth_device_names(ModelRc::new(VecModel::from(
        bluetooth_device_row_labels(devices),
    )));

    if devices.is_empty() {
        app.set_bluetooth_device_selected_index(0);
        app.set_bluetooth_device_selected_paired(false);
        app.set_bluetooth_pairing_status(
            "No PS5 DualSense device found. Press Scan devices to search again.".into(),
        );
        app.set_bluetooth_device_list_text(format_bluetooth_devices(devices, 0).into());
        return;
    }

    let selected_index = selected_index.min(devices.len() - 1);
    let selected = &devices[selected_index];
    app.set_bluetooth_device_selected_index(selected_index as i32);
    app.set_bluetooth_device_selected_paired(selected.paired);
    app.set_bluetooth_device_list_text(format_bluetooth_devices(devices, selected_index).into());
    app.set_bluetooth_pairing_status(if selected.paired {
        format!(
            "{} is already paired. Press Continue to start button mapping.",
            selected.name
        )
        .into()
    } else {
        format!(
            "{} is ready. Press Continue to pair and start button mapping.",
            selected.name
        )
        .into()
    });
}

fn start_bluetooth_pairing_light_blink(app_weak: slint::Weak<Demo>) {
    BLUETOOTH_PAIRING_LIGHT_TIMER.with(|timer_slot| {
        let mut timer_slot = timer_slot.borrow_mut();
        let timer = timer_slot.get_or_insert_with(slint::Timer::default);
        timer.stop();
        timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            if app.get_screen() != 8 || !app.get_bluetooth_pairing_intro() {
                return;
            }

            app.set_bluetooth_link_light_off_visible(!app.get_bluetooth_link_light_off_visible());
        });
    });
}

fn stop_bluetooth_pairing_light_blink() {
    BLUETOOTH_PAIRING_LIGHT_TIMER.with(|timer_slot| {
        if let Some(timer) = timer_slot.borrow().as_ref() {
            timer.stop();
        }
    });
}

fn update_system_settings(
    app: &Demo,
    mut update: impl FnMut(&mut SystemSettings),
) -> Result<SystemSettings, String> {
    let mut settings = load_or_create_system_settings()?;
    update(&mut settings);
    save_system_settings(&settings)?;
    apply_system_settings_to_app(app, &settings);
    Ok(settings)
}

fn refresh_controller_setup_view(
    app: &Demo,
    setup_state: &Arc<Mutex<Option<ControllerSetupState>>>,
) {
    let guard = setup_state.lock().expect("setup_state lock poisoned");
    let Some(state) = guard.as_ref() else {
        return;
    };

    app.set_controller_config_title(state.title().into());
    app.set_controller_config_progress(state.progress_text().into());
    app.set_controller_config_mappings(state.mapping_lines().into());
    app.set_controller_config_instruction(state.instruction_text().into());
    app.set_controller_config_art_stage(state.artwork_stage());
    app.set_controller_config_complete(state.is_complete());
    app.set_controller_device_selection(state.is_device_selection());
    app.set_controller_device_list_text(state.device_list_text().into());
    app.set_controller_device_names(ModelRc::new(VecModel::from(
        state
            .device_labels()
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    app.set_controller_device_selected_index(state.selected_device_index() as i32);
    app.set_controller_config_artwork_set(state.artwork_set().into());
    app.set_controller_config_current_button(state.current_button_label().into());
    app.set_controller_config_overlay_name(
        state.current_overlay_name().unwrap_or("").into(),
    );
    app.set_controller_config_progress_percent(state.current_progress_percent());
}

fn play_dualsense_rotation_animation(
    app: &Demo,
    setup_state: &Arc<Mutex<Option<ControllerSetupState>>>,
) {
    {
        let mut guard = setup_state.lock().expect("setup_state lock poisoned");
        let Some(state) = guard.as_mut() else {
            return;
        };
        if !state.should_play_rotation_animation() {
            return;
        }
        state.mark_rotation_played();
    }

    app.set_controller_config_art_stage(1);
    schedule_dualsense_rotation_frame(app.as_weak(), Arc::clone(setup_state), 2);
}

fn schedule_dualsense_rotation_frame(
    app_weak: slint::Weak<Demo>,
    setup_state: Arc<Mutex<Option<ControllerSetupState>>>,
    stage: i32,
) {
    let app_weak_next = app_weak.clone();
    let setup_state_next = Arc::clone(&setup_state);
    slint::Timer::single_shot(Duration::from_millis(120), move || {
        let Some(app) = app_weak_next.upgrade() else {
            return;
        };

        let setup_active = {
            let guard = setup_state_next.lock().expect("setup_state lock poisoned");
            guard.as_ref().is_some_and(|state| state.is_mapping_controls())
        };
        if !setup_active {
            return;
        }

        app.set_controller_config_art_stage(stage);

        if stage < 18 {
            schedule_dualsense_rotation_frame(app.as_weak(), Arc::clone(&setup_state_next), stage + 1);
        } else {
            refresh_controller_setup_view(&app, &setup_state_next);
        }
    });
}

fn begin_controller_setup(
    app: &Demo,
    setup_state: &Arc<Mutex<Option<ControllerSetupState>>>,
    template_index: i32,
) {
    let _ = send_runtime_mapper_toggle_request(&supervisor_socket_path(), false);

    let mut state = ControllerSetupState::new(template_index);
    log_launch(&format!(
        "controller setup: template_index={} device_selection={} device_count={} selected_index={}",
        template_index,
        state.is_device_selection(),
        state.device_count(),
        state.selected_device_index()
    ));
    if template_index == 2 {
        let bluetooth_address = app.get_paired_bluetooth_address();
        if !bluetooth_address.is_empty() {
            state.set_bluetooth_address(Some(bluetooth_address.to_string()));
            state.skip_device_selection_for_dualsense();
        }
    }

    app.set_controller_config_last_input("-".into());
    app.set_controller_capture_armed(false);
    app.set_controller_config_complete(false);
    app.set_controller_final_selection(0);
    app.set_controller_config_art_stage(0);
    app.set_controller_device_selection(state.is_device_selection());
    app.set_controller_config_artwork_set(state.artwork_set().into());
    app.set_controller_config_current_button("".into());
    app.set_controller_config_overlay_name("".into());
    app.set_controller_config_progress_percent(0);

    *setup_state.lock().expect("setup_state lock poisoned") = Some(state);
    refresh_controller_setup_view(app, setup_state);
}

fn finalize_controller_setup(
    app: &Demo,
    setup_state: &Arc<Mutex<Option<ControllerSetupState>>>,
    selected_index: &Arc<Mutex<usize>>,
    confirm: bool,
) {
    log_launch(&format!(
        "controller setup: finalize requested confirm={confirm} screen={}",
        app.get_screen()
    ));

    if !confirm {
        app.set_controller_manager_status("Controller setup cancelled.".into());
        app.set_controller_config_last_input("Cancelled".into());
        app.set_controller_config_current_button("".into());
        app.set_controller_config_overlay_name("".into());
        app.set_controller_config_progress_percent(0);
        *setup_state.lock().expect("setup_state lock poisoned") = None;
        thread::spawn(|| {
            if let Err(error) = send_runtime_mapper_toggle_request(&supervisor_socket_path(), true)
            {
                log_launch(&format!(
                    "failed to re-enable runtime mapper after controller setup cancel: {error}"
                ));
            }
        });
        return;
    }

    let state = {
        let guard = setup_state.lock().expect("setup_state lock poisoned");
        guard.clone()
    };

    let Some(state) = state else {
        log_launch("controller setup: finalize aborted, no setup state");
        app.set_controller_manager_status("No controller setup in progress.".into());
        return;
    };

    if !state.is_complete() {
        log_launch("controller setup: finalize aborted, mapping not complete");
        app.set_controller_manager_status("Controller mapping is not complete.".into());
        return;
    }

    let mut store = match load_store() {
        Ok(store) => store,
        Err(error) => {
            app.set_controller_manager_status(
                format!("Failed to load controller store: {error}").into(),
            );
            return;
        }
    };

    let profile = state.to_profile(store.controllers.len());
    log_launch(&format!(
        "controller setup: saving profile id={} name={}",
        profile.id, profile.name
    ));
    store.controllers.push(profile);

    match save_store(&store) {
        Ok(()) => {
            {
                let mut selected = selected_index.lock().expect("selected_index lock poisoned");
                *selected = store.controllers.len().saturating_sub(1);
            }

            *setup_state.lock().expect("setup_state lock poisoned") = None;

            app.set_controller_manager_status(
                "Controller saved to /data/openconsole/controllers.json".into(),
            );
            app.set_controller_config_last_input("Saved".into());
            app.set_controller_config_current_button("".into());
            app.set_controller_config_overlay_name("".into());
            app.set_controller_config_progress_percent(0);
            app.set_screen(1);
            app.set_focus_region(1);
            refresh_controller_list(app, selected_index);
            log_launch("controller setup: save complete, returned to games view");

            thread::spawn(|| {
                if let Err(error) = send_runtime_mapper_toggle_request(&supervisor_socket_path(), true)
                {
                    log_launch(&format!(
                        "failed to re-enable runtime mapper after controller setup save: {error}"
                    ));
                }
            });
        }
        Err(error) => {
            log_launch(&format!("failed to save controller: {error}"));
            app.set_controller_manager_status(format!("Failed to save controller: {error}").into());
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    std::panic::set_hook(Box::new(|panic_info| {
        log_launch(&format!("ui panic: {panic_info}"));
    }));

    log_launch("ui startup v2");

    let app = Demo::new()?;
    let runtime_catalog = Arc::new(load_runtime_game_catalog(&app).expect("failed to build runtime game catalog"));
    let detail_image_cache = Arc::new(Mutex::new(GameDetailImageCache::default()));
    let game_state = Arc::new(Mutex::new(load_game_state().unwrap_or_default()));
    apply_manifest_to_ui(&app, &runtime_catalog);
    let app_launch_weak = app.as_weak();
    let setup_state: Arc<Mutex<Option<ControllerSetupState>>> = Arc::new(Mutex::new(None));
    let selected_index: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let bluetooth_devices: Arc<Mutex<Vec<BluetoothDeviceInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let bluetooth_debug_state: Arc<Mutex<BluetoothDebugState>> =
        Arc::new(Mutex::new(BluetoothDebugState::default()));
    let wifi_networks: Arc<Mutex<Vec<WifiNetwork>>> = Arc::new(Mutex::new(Vec::new()));
    let wifi_password_state: Arc<Mutex<WifiPasswordEditorState>> =
        Arc::new(Mutex::new(WifiPasswordEditorState::default()));

    let _input_handle = {
        let app_weak = app.as_weak();
        let setup_state = Arc::clone(&setup_state);
        let selected_index = Arc::clone(&selected_index);
        let wifi_password_state = Arc::clone(&wifi_password_state);
        start_console_input(move |event| {
            let app_weak = app_weak.clone();
            let setup_state = Arc::clone(&setup_state);
            let selected_index = Arc::clone(&selected_index);
            let wifi_password_state = Arc::clone(&wifi_password_state);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app_weak.upgrade() {
                    handle_console_input_event(
                        &app,
                        &event,
                        &setup_state,
                        &selected_index,
                        &wifi_password_state,
                    );
                }
            });
        })
        .expect("failed to start SDL input pump")
    };

    refresh_controller_list(&app, &selected_index);
    refresh_system_settings(&app);
    {
        let game_state = game_state.lock().expect("game_state lock poisoned").clone();
        refresh_home_view(&app, &runtime_catalog, &game_state);
        refresh_selected_game_view(&app, &runtime_catalog, &game_state, &detail_image_cache);
    }

    {
        let app_weak = app.as_weak();
        let wifi_networks = Arc::clone(&wifi_networks);
        thread::spawn(move || loop {
            let app_weak_inner = app_weak.clone();
            let wifi_networks_inner = Arc::clone(&wifi_networks);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = app_weak_inner.upgrade() else {
                    return;
                };

                poll_system_settings(&app);
                refresh_wifi_setup_screen(&app, &wifi_networks_inner);
            });

            thread::sleep(Duration::from_secs(3));
        });
    }

    if let Ok(store) = load_store() {
        if app.get_bluetooth_enabled() {
            let _ = reconnect_saved_dualsense_devices(&store);
        }
    }

    let runtime_catalog_for_launch = Arc::clone(&runtime_catalog);
    let game_state_for_launch = Arc::clone(&game_state);
    app.on_launch_game(move |game_index| {
        let Some(app) = app_launch_weak.upgrade() else {
            return;
        };

        if app.get_game_launching() {
            log_launch("ui ignored duplicate launch request");
            return;
        }

        let Some(game) = runtime_catalog_for_launch
            .manifest
            .game_by_index(game_index as usize)
        else {
            app.set_launch_status("Launch failed: game not found".into());
            return;
        };

        {
            let mut guard = game_state_for_launch
                .lock()
                .expect("game_state lock poisoned");
            guard.record_recently_played(&game.id);
            if let Err(error) = save_game_state(&guard) {
                let path = game_state_path();
                log_launch(&format!(
                    "failed to save recently played games to {}: {error}",
                    path.display()
                ));
                app.set_launch_status(
                    format!("Failed to save recent games to {}: {error}", path.display()).into(),
                );
                return;
            }
            refresh_home_view(&app, &runtime_catalog_for_launch, &guard);
        }

        log_launch(&format!(
            "ui sending launch request for game index {}",
            game_index
        ));
        app.set_game_launching(true);
        app.set_launch_status("Requesting game launch...".into());

        let socket_path = supervisor_socket_path();
        match send_launch_request(&socket_path, game_index as usize) {
            Ok(response) if response.ok => {
                log_launch(&format!(
                    "ui launch request accepted for game index {} via {}",
                    game_index,
                    socket_path.display()
                ));
                let _ = app.hide();
                let _ = slint::quit_event_loop();
            }
            Ok(response) => {
                log_launch(&format!("ui launch request rejected: {}", response.message));
                app.set_game_launching(false);
                app.set_launch_status(response.message.into());
            }
            Err(error) => {
                log_launch(&format!("ui launch request failed: {error}"));
                app.set_game_launching(false);
                app.set_launch_status(format!("Launch failed: {error}").into());
            }
        }
    });

    {
        let app_weak = app.as_weak();
        let runtime_catalog = Arc::clone(&runtime_catalog);
        let game_state = Arc::clone(&game_state);
        let detail_image_cache = Arc::clone(&detail_image_cache);
        app.on_select_game(move |game_index| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let selected_game = game_index.max(0) as usize;
            app.set_selected_game(selected_game as i32);
            app.set_selected_screenshot_index(0);
            app.set_screenshot_popup_open(false);

            let state = game_state.lock().expect("game_state lock poisoned").clone();
            refresh_selected_game_view(&app, &runtime_catalog, &state, &detail_image_cache);
        });
    }

    {
        let app_weak = app.as_weak();
        let runtime_catalog = Arc::clone(&runtime_catalog);
        let game_state = Arc::clone(&game_state);
        app.on_toggle_game_favorite(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let selected_game = app.get_selected_game().max(0) as usize;
            let Some(game) = runtime_catalog.manifest.game_by_index(selected_game) else {
                return;
            };

            let mut guard = game_state.lock().expect("game_state lock poisoned");
            let _ = guard.toggle_favorite(&game.id);
            if let Err(error) = save_game_state(&guard) {
                app.set_launch_status(format!("Failed to save favorites: {error}").into());
            }

            app.set_game_favorite(guard.is_favorite(&game.id));
            refresh_home_view(&app, &runtime_catalog, &guard);
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_select_game_screenshot(move |index| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let screenshot_count = app.get_game_screenshots().row_count() as i32;
            let selected_index = if screenshot_count <= 0 {
                0
            } else {
                index.max(0).min(screenshot_count - 1)
            };
            app.set_selected_detail_control(3);
            app.set_selected_screenshot_index(selected_index);
            app.set_screenshot_popup_open(true);
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_restart_system(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            log_launch("ui sending reboot request to supervisor");
            let socket_path = supervisor_socket_path();
            match send_reboot_request(&socket_path) {
                Ok(response) if response.ok => {
                    log_launch(&format!(
                        "ui reboot request accepted via {}",
                        socket_path.display()
                    ));
                    let _ = app.hide();
                    let _ = slint::quit_event_loop();
                }
                Ok(response) => {
                    log_launch(&format!("ui reboot request rejected: {}", response.message));
                }
                Err(error) => {
                    log_launch(&format!("ui reboot request failed: {error}"));
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_toggle_wifi(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let result = update_system_settings(&app, |settings| {
                settings.wifi_enabled = !settings.wifi_enabled;
            });

            match result {
                Ok(settings) => {
                    let enabling = settings.wifi_enabled;
                    let apply_settings = settings.clone();
                    let app_weak_thread = app.as_weak();

                    apply_system_settings_to_app(&app, &settings);
                    app.set_system_settings_status(
                        if enabling {
                            "Starting WiFi service..."
                        } else {
                            "Stopping WiFi service..."
                        }
                        .into(),
                    );

                    thread::spawn(move || {
                        let apply_result = apply_system_settings(&apply_settings);
                        let _ = slint::invoke_from_event_loop(move || {
                            let Some(app) = app_weak_thread.upgrade() else {
                                return;
                            };

                            match apply_result {
                                Ok(()) => {
                                    if apply_settings.wifi_enabled {
                                        let _ = enforce_connectivity_policy(
                                            apply_settings.wifi_enabled,
                                            apply_settings.preferred_wifi_network.as_deref(),
                                        );
                                    }
                                    apply_system_settings_to_app(&app, &apply_settings);
                                    app.set_system_settings_status(
                                        if apply_settings.wifi_enabled {
                                            "WiFi enabled"
                                        } else {
                                            "WiFi disabled"
                                        }
                                        .into(),
                                    );
                                }
                                Err(error) => {
                                    app.set_system_settings_status(
                                        format!("WiFi saved, but applying failed: {error}")
                                            .into(),
                                    );
                                }
                            }
                        });
                    });
                }
                Err(error) => {
                    app.set_system_settings_status(
                        format!("Failed to save WiFi setting: {error}").into(),
                    );
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_open_wifi_setup(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let return_screen = match app.get_screen() {
                7 => 12,
                12 => 12,
                _ => 12,
            };
            app.set_wifi_setup_return_screen(return_screen);
            app.set_screen(9);
            app.set_focus_region(1);
            app.set_wifi_network_action(1);

            if !app.get_wifi_enabled() {
                app.set_wifi_network_placeholder_text(
                    "Wi-Fi is disabled. Enable it in System Settings to scan for networks."
                        .into(),
                );
                app.set_wifi_network_status("".into());
                return;
            }

            app.set_wifi_network_placeholder_text("Searching for networks...".into());
            app.set_wifi_network_status("".into());
            app.invoke_scan_wifi_networks();
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_networks_for_scan = Arc::clone(&wifi_networks);
        app.on_scan_wifi_networks(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            if !app.get_wifi_enabled() {
                app.set_wifi_network_placeholder_text(
                    "Wi-Fi is disabled. Enable it in System Settings to scan for networks."
                        .into(),
                );
                app.set_wifi_network_status("".into());
                return;
            }

            app.set_wifi_network_placeholder_text("Searching for networks...".into());
            app.set_wifi_network_status("".into());

            let app_weak_thread = app.as_weak();
            let wifi_networks_thread = Arc::clone(&wifi_networks_for_scan);
            thread::spawn(move || {
                let scan_result = scan_networks();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak_thread.upgrade() else {
                        return;
                    };

                    match scan_result {
                        Ok(networks) => {
                            {
                                let mut guard = wifi_networks_thread
                                    .lock()
                                    .expect("wifi networks lock poisoned");
                                *guard = networks;
                                refresh_network_status(&mut guard);
                            }

                            let guard = wifi_networks_thread
                                .lock()
                                .expect("wifi networks lock poisoned");
                            sync_wifi_network_selection(&app, &guard, 0);
                        }
                        Err(error) => {
                            app.set_wifi_network_placeholder_text(
                                format!("Wi-Fi scan failed: {error}").into(),
                            );
                            app.set_wifi_network_status("".into());
                        }
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_networks_for_select = Arc::clone(&wifi_networks);
        app.on_select_wifi_network(move |index| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let guard = wifi_networks_for_select
                .lock()
                .expect("wifi networks lock poisoned");
            if guard.is_empty() {
                app.set_wifi_network_selected_index(0);
                return;
            }

            let selected_index = (index as usize).min(guard.len() - 1);
            sync_wifi_network_selection(&app, &guard, selected_index);
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_networks = Arc::clone(&wifi_networks);
        let wifi_password_state = Arc::clone(&wifi_password_state);
        app.on_activate_wifi_network(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let selected_index = app.get_wifi_network_selected_index().max(0) as usize;
            let selected_network = {
                let guard = wifi_networks.lock().expect("wifi networks lock poisoned");
                guard.get(selected_index).cloned()
            };

            let Some(network) = selected_network else {
                app.set_wifi_network_status("No Wi-Fi network selected.".into());
                return;
            };

            if network.known {
                let _ = disconnect_wifi();
                match update_system_settings(&app, |settings| {
                    settings.preferred_wifi_network = None;
                }) {
                    Ok(settings) => {
                        let _ = apply_system_settings(&settings);
                        app.set_wifi_network_status(
                            format!("Removed Wi-Fi configuration for {}.", network.ssid).into(),
                        );
                        app.set_system_settings_status(
                            format!("Wi-Fi configuration removed: {}", network.ssid).into(),
                        );
                        refresh_wifi_setup_screen(&app, &wifi_networks);
                    }
                    Err(error) => {
                        app.set_wifi_network_status(
                            format!("Failed to remove Wi-Fi configuration: {error}").into(),
                        );
                    }
                }
                return;
            }

            if network.secured && !network.known {
                open_wifi_password_editor(
                    &app,
                    &wifi_password_state,
                    &network.ssid,
                    "Enter the Wi-Fi password, then choose Connect & Save.",
                );
                return;
            }

            app.set_wifi_network_status(format!("Connecting to {}...", network.ssid).into());
            match connect_to_network(&network.ssid, None) {
                Ok(()) => match persist_wifi_selection(&app, &network.ssid) {
                    Ok(_) => {
                        app.set_wifi_network_status(
                            format!("Connected to {}. Wi-Fi network saved.", network.ssid).into(),
                        );
                        app.set_system_settings_status(
                            format!("Wi-Fi network saved: {}", network.ssid).into(),
                        );
                        app.set_screen(9);
                        app.set_focus_region(1);
                        app.set_wifi_network_action(1);
                    }
                    Err(error) => {
                        app.set_wifi_network_status(
                            format!("Connected, but saving failed: {error}").into(),
                        );
                    }
                },
                Err(error) => {
                    if network.secured {
                        open_wifi_password_editor(
                            &app,
                            &wifi_password_state,
                            &network.ssid,
                            &format!(
                                "Saved Wi-Fi password failed: {error}. Enter a new password."
                            ),
                        );
                    } else {
                        app.set_wifi_network_status(
                            format!("Failed to connect to {}: {error}", network.ssid).into(),
                        );
                    }
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_cancel_wifi_setup(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            app.set_screen(app.get_wifi_setup_return_screen());
            app.set_focus_region(1);
            if app.get_wifi_setup_return_screen() == 7 {
                app.set_system_setting(1);
            } else if app.get_wifi_setup_return_screen() == 12 {
                app.set_network_options_selected_index(1);
            } else {
                app.set_selected_setting(1);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_password_state = Arc::clone(&wifi_password_state);
        app.on_wifi_password_backspace(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let mut guard = wifi_password_state
                .lock()
                .expect("wifi password state lock poisoned");
            if guard.password.pop().is_some() {
                sync_wifi_password_editor(&app, &guard, "Removed the last character.");
            } else {
                sync_wifi_password_editor(&app, &guard, "Password is already empty.");
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_password_state = Arc::clone(&wifi_password_state);
        app.on_wifi_password_select_key(move |key| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let mut guard = wifi_password_state
                .lock()
                .expect("wifi password state lock poisoned");
            guard.selected_key = key.to_string();
            sync_wifi_password_editor(&app, &guard, app.get_wifi_password_status().as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_password_state = Arc::clone(&wifi_password_state);
        app.on_wifi_password_append_selected(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

                let key = app.get_wifi_password_key_selected().to_string();
                if key == "Delete" {
                    let mut guard = wifi_password_state
                        .lock()
                        .expect("wifi password state lock poisoned");
                    guard.password.pop();
                    sync_wifi_password_editor(&app, &guard, "Removed the last character.");
                    return;
                }

                let text = if key == "Space" { " " } else { key.as_str() };
                append_wifi_password_text(
                    &app,
                    &wifi_password_state,
                    text,
                    "Password updated. Choose Connect & Save when ready.",
                );
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_password_state = Arc::clone(&wifi_password_state);
        app.on_wifi_password_select_mode(move |mode| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let mut guard = wifi_password_state
                .lock()
                .expect("wifi password state lock poisoned");
            wifi_password_set_keyboard_mode(&mut guard, mode);
            app.set_wifi_password_action(wifi_password_action_for_mode(guard.keyboard_mode));
            sync_wifi_password_editor(&app, &guard, app.get_wifi_password_status().as_str());
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_password_state = Arc::clone(&wifi_password_state);
        app.on_wifi_password_append_text(move |text| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            append_wifi_password_text(
                &app,
                &wifi_password_state,
                text.as_str(),
                "Password updated from keyboard. Choose Connect & Save when ready.",
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_password_state = Arc::clone(&wifi_password_state);
        let wifi_password_state = Arc::clone(&wifi_password_state);
        app.on_wifi_password_toggle_visibility(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let mut guard = wifi_password_state
                .lock()
                .expect("wifi password state lock poisoned");
            guard.show_password = !guard.show_password;
            sync_wifi_password_editor(
                &app,
                &guard,
                if guard.show_password {
                    "Password is now visible."
                } else {
                    "Password is now hidden."
                },
            );
        });
    }

    {
        let app_weak = app.as_weak();
        let wifi_password_state = Arc::clone(&wifi_password_state);
        let wifi_networks = Arc::clone(&wifi_networks);
        app.on_wifi_password_connect(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let (ssid, password) = {
                let guard = wifi_password_state
                    .lock()
                    .expect("wifi password state lock poisoned");
                (guard.ssid.clone(), guard.password.clone())
            };

            if ssid.is_empty() {
                app.set_wifi_password_status("No Wi-Fi network is selected.".into());
                return;
            }

            if password.is_empty() {
                app.set_wifi_password_status("Enter the Wi-Fi password first.".into());
                return;
            }

            app.set_wifi_password_status(format!("Connecting to {}...", ssid).into());
            match connect_to_network(&ssid, Some(&password)) {
                Ok(()) => match persist_wifi_selection(&app, &ssid) {
                    Ok(_) => {
                        let ethernet_active = enforce_connectivity_policy(true, Some(&ssid))
                            .map(|status| status.ethernet_active)
                            .unwrap_or(false);
                        refresh_system_settings(&app);
                        clear_wifi_password_editor(
                            &app,
                            &wifi_password_state,
                            "Wi-Fi password cleared.",
                        );
                        let status_message = if ethernet_active {
                            format!(
                                "Wi-Fi network saved: {ssid}. Ethernet is active, so Wi-Fi will reconnect after the network cable is unplugged."
                            )
                        } else {
                            format!("Wi-Fi network saved: {ssid}")
                        };
                        app.set_system_settings_status(status_message.clone().into());
                        app.set_wifi_network_status(status_message.into());
                        let guard = wifi_networks
                            .lock()
                            .expect("wifi networks lock poisoned");
                        if !guard.is_empty() {
                            let selected_index = app.get_wifi_network_selected_index().max(0) as usize;
                            sync_wifi_network_selection(&app, &guard, selected_index);
                        }
                        app.invoke_cancel_wifi_setup();
                    }
                    Err(error) => {
                        app.set_wifi_password_status(
                            format!("Connected, but saving failed: {error}").into(),
                        );
                    }
                },
                Err(error) => {
                    app.set_wifi_password_status(
                        format!("Failed to connect to {}: {error}", ssid).into(),
                    );
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_toggle_bluetooth(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let result = update_system_settings(&app, |settings| {
                settings.bluetooth_enabled = !settings.bluetooth_enabled;
            });

            match result {
                Ok(settings) => {
                    if let Err(error) = apply_system_settings(&settings) {
                        app.set_system_settings_status(
                            format!("Bluetooth saved, but applying failed: {error}").into(),
                        );
                    } else {
                        let status = if settings.bluetooth_enabled {
                            "Bluetooth enabled"
                        } else {
                            "Bluetooth disabled"
                        };
                        app.set_system_settings_status(status.into());
                    }
                }
                Err(error) => {
                    app.set_system_settings_status(
                        format!("Failed to save Bluetooth setting: {error}").into(),
                    );
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let selected_index = Arc::clone(&selected_index);
        app.on_refresh_controllers(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            refresh_controller_list(&app, &selected_index);
        });
    }

    {
        let app_weak = app.as_weak();
        let selected_index = Arc::clone(&selected_index);
        app.on_controller_select_prev(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let store = match load_store() {
                Ok(store) => store,
                Err(error) => {
                    app.set_controller_manager_status(
                        format!("Failed to load controllers: {error}").into(),
                    );
                    return;
                }
            };

            let mut selected = selected_index.lock().expect("selected_index lock poisoned");
            if *selected > 0 {
                *selected -= 1;
            } else if store.controllers.is_empty() {
                *selected = 0;
            }
            drop(selected);
            refresh_controller_list(&app, &selected_index);
        });
    }

    {
        let app_weak = app.as_weak();
        let selected_index = Arc::clone(&selected_index);
        app.on_controller_select_next(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let store = match load_store() {
                Ok(store) => store,
                Err(error) => {
                    app.set_controller_manager_status(
                        format!("Failed to load controllers: {error}").into(),
                    );
                    return;
                }
            };

            let mut selected = selected_index.lock().expect("selected_index lock poisoned");
            if *selected < store.controllers.len() {
                *selected += 1;
            }
            drop(selected);
            refresh_controller_list(&app, &selected_index);
        });
    }

    {
        let app_weak = app.as_weak();
        let selected_index = Arc::clone(&selected_index);
        app.on_remove_selected_controller(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let mut store = match load_store() {
                Ok(store) => store,
                Err(error) => {
                    app.set_controller_manager_status(
                        format!("Failed to load controller store: {error}").into(),
                    );
                    return;
                }
            };

            if store.controllers.is_empty() {
                app.set_controller_manager_status("No controllers to remove.".into());
                return;
            }

            let idx = *selected_index.lock().expect("selected_index lock poisoned");
            if idx >= store.controllers.len() {
                app.set_controller_manager_status("Select a controller to remove first.".into());
                return;
            }

            let remove_idx = idx.min(store.controllers.len() - 1);
            let removed_controller = store.controllers[remove_idx].clone();
            let mut removal_status_note: Option<String> = None;
            if removed_controller.template == "ps5-dualsense" {
                if let Some(address) = removed_controller.bluetooth_address.as_deref() {
                    if let Err(error) = unpair_dualsense_device(address) {
                        removal_status_note = Some(format!(
                            "Removed controller config, but unpair failed for {}: {error}",
                            removed_controller.name
                        ));
                    }

                    if app.get_paired_bluetooth_address() == address {
                        app.set_paired_bluetooth_address("".into());
                    }
                }
            }

            let removed_name = removed_controller.name.clone();
            store.controllers.remove(remove_idx);

            if let Err(error) = save_store(&store) {
                app.set_controller_manager_status(
                    format!("Failed to remove controller: {error}").into(),
                );
                return;
            }

            {
                let mut selected = selected_index.lock().expect("selected_index lock poisoned");
                if store.controllers.is_empty() {
                    *selected = 0;
                } else if *selected >= store.controllers.len() {
                    *selected = store.controllers.len();
                }
            }

            app.set_controller_manager_status(
                removal_status_note
                    .unwrap_or_else(|| format!("Removed controller: {removed_name}"))
                    .into(),
            );
            refresh_controller_list(&app, &selected_index);
        });
    }

    {
        let app_weak = app.as_weak();
        let setup_state = Arc::clone(&setup_state);
        app.on_start_controller_setup(move |template_index| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            begin_controller_setup(&app, &setup_state, template_index);
        });
    }

    {
        let app_weak = app.as_weak();
        let bluetooth_devices_for_scan = Arc::clone(&bluetooth_devices);
        let bluetooth_debug_state_for_scan = Arc::clone(&bluetooth_debug_state);
        app.on_scan_bluetooth_devices(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            stop_bluetooth_pairing_light_blink();
            app.set_bluetooth_pairing_intro(false);
            app.set_bluetooth_scan_running(true);
            app.set_bluetooth_link_light_off_visible(false);
            app.set_bluetooth_pairing_status("Started scanning Bluetooth devices...".into());
            app.set_bluetooth_device_list_text("Scanning for DualSense devices...".into());
            app.set_bluetooth_device_action(BLUETOOTH_ACTION_PRIMARY_BUTTON);
            set_bluetooth_debug_report(
                &app,
                &bluetooth_debug_state_for_scan,
                "Started scanning Bluetooth devices...\nWaiting for bluetoothctl output...".to_string(),
            );

            let app_weak_thread = app.as_weak();
            let bluetooth_devices_thread = Arc::clone(&bluetooth_devices_for_scan);
            let bluetooth_debug_state_thread = Arc::clone(&bluetooth_debug_state_for_scan);
            thread::spawn(move || {
                let scan_result = scan_dualsense_devices();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak_thread.upgrade() else {
                        return;
                    };

                    match scan_result {
                        Ok(report) => {
                            app.set_bluetooth_scan_running(false);
                            let selected_index = 0usize;
                            let dualsense_devices = report.dualsense_devices.clone();

                            let debug_report = format_bluetooth_scan_debug(&report, selected_index);
                            log_launch(&debug_report);
                            set_bluetooth_debug_report(&app, &bluetooth_debug_state_thread, debug_report);

                            {
                                let mut guard = bluetooth_devices_thread
                                    .lock()
                                    .expect("bluetooth_devices lock poisoned");
                                *guard = dualsense_devices;
                            }

                            let guard = bluetooth_devices_thread
                                .lock()
                                .expect("bluetooth_devices lock poisoned");
                            sync_bluetooth_selection(&app, &guard, selected_index);

                            if guard.is_empty() {
                                app.set_bluetooth_pairing_status(
                                    format!(
                                        "End scanning. No DualSense device found. {} Bluetooth device(s) seen.",
                                        report.all_devices.len()
                                    )
                                    .into(),
                                );
                            } else {
                                app.set_bluetooth_device_action(BLUETOOTH_ACTION_DEVICE_LIST);
                                app.set_bluetooth_pairing_status(
                                    format!(
                                        "End scanning. Found {} DualSense device(s) among {} Bluetooth device(s).",
                                        guard.len(),
                                        report.all_devices.len()
                                    )
                                    .into(),
                                );
                            }

                        }
                        Err(error) => {
                            app.set_bluetooth_scan_running(false);
                            app.set_bluetooth_pairing_status(
                                format!("Bluetooth scan failed: {error}").into(),
                            );
                            set_bluetooth_debug_report(
                                &app,
                                &bluetooth_debug_state_thread,
                                format!(
                                    "Started scanning Bluetooth devices...\n\nScan failed:\n{error}"
                                ),
                            );
                        }
                    }
                });
            });
        });

        {
            let app_weak = app.as_weak();
            let bluetooth_devices_for_select = Arc::clone(&bluetooth_devices);
            app.on_select_bluetooth_device(move |index| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };

                let guard = bluetooth_devices_for_select
                    .lock()
                    .expect("bluetooth_devices lock poisoned");
                if guard.is_empty() {
                    app.set_bluetooth_device_selected_index(0);
                    app.set_bluetooth_device_selected_paired(false);
                    return;
                }

                let selected_index = (index as usize).min(guard.len() - 1);
                sync_bluetooth_selection(&app, &guard, selected_index);
            });
        }
    }

    {
        let app_weak = app.as_weak();
        let bluetooth_devices = Arc::clone(&bluetooth_devices);
        let setup_state = Arc::clone(&setup_state);
        app.on_pair_or_continue_bluetooth_device(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let selected_index = app.get_bluetooth_device_selected_index().max(0) as usize;
            let selected_device = {
                let guard = bluetooth_devices
                    .lock()
                    .expect("bluetooth_devices lock poisoned");
                guard.get(selected_index).cloned()
            };

            let Some(device) = selected_device else {
                app.set_bluetooth_pairing_status("No DualSense controller selected.".into());
                return;
            };

            app.set_paired_bluetooth_address(device.address.clone().into());

            let app_weak_thread = app.as_weak();
            let bluetooth_devices_thread = Arc::clone(&bluetooth_devices);
            let setup_state_thread = Arc::clone(&setup_state);
            thread::spawn(move || {
                let device_result = if device.paired {
                    connect_dualsense_device(&device.address)
                        .or_else(|_| pair_dualsense_device(&device.address))
                } else {
                    pair_dualsense_device(&device.address)
                };
                let pair_error = device_result.as_ref().err().cloned();
                let ready_device = device_result.unwrap_or_else(|_| BluetoothDeviceInfo {
                    address: device.address.clone(),
                    name: device.name.clone(),
                    paired: true,
                    trusted: true,
                    connected: true,
                });

                let mut enumerated = false;
                for _ in 0..BLUETOOTH_MAPPING_ENUMERATION_RETRY_COUNT {
                    let devices = list_all_controller_input_devices();
                    if devices.iter().any(|candidate| {
                        let lower_name = candidate.name.to_ascii_lowercase();
                        lower_name.contains("dualsense") || lower_name.contains("wireless controller")
                    }) {
                        enumerated = true;
                        break;
                    }
                    thread::sleep(BLUETOOTH_MAPPING_ENUMERATION_RETRY_DELAY);
                }

                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak_thread.upgrade() else {
                        return;
                    };

                    {
                        let mut guard = bluetooth_devices_thread
                            .lock()
                            .expect("bluetooth_devices lock poisoned");
                        if let Some(existing) = guard.get_mut(selected_index) {
                            *existing = ready_device.clone();
                        }
                    }

                    let guard = bluetooth_devices_thread
                        .lock()
                        .expect("bluetooth_devices lock poisoned");
                    sync_bluetooth_selection(&app, &guard, selected_index);

                    if !enumerated {
                        if let Some(error) = pair_error {
                            app.set_bluetooth_pairing_status(
                                format!("Failed to prepare {}: {error}", device.name).into(),
                            );
                        } else {
                            app.set_bluetooth_pairing_status(
                                format!(
                                    "{} connected, but controller input did not appear yet. Try Scan / Refresh again.",
                                    ready_device.name
                                )
                                .into(),
                            );
                        }
                        return;
                    }

                    app.set_bluetooth_pairing_status(
                        format!("{} ready. Opening controller mapping.", ready_device.name).into(),
                    );
                    app.set_screen(6);
                    app.set_bluetooth_scan_running(false);
                    app.set_bluetooth_pairing_intro(false);
                    begin_controller_setup(&app, &setup_state_thread, 2);
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let bluetooth_devices = Arc::clone(&bluetooth_devices);
        app.on_unpair_bluetooth_device(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let selected_index = app.get_bluetooth_device_selected_index().max(0) as usize;
            let selected_device = {
                let guard = bluetooth_devices
                    .lock()
                    .expect("bluetooth_devices lock poisoned");
                guard.get(selected_index).cloned()
            };

            let Some(device) = selected_device else {
                app.set_bluetooth_pairing_status("No DualSense controller selected.".into());
                return;
            };

            if !device.paired {
                app.set_bluetooth_pairing_status(
                    format!("{} is not paired.", device.name).into(),
                );
                return;
            }

            match unpair_dualsense_device(&device.address) {
                Ok(()) => {
                    {
                        let mut guard = bluetooth_devices
                            .lock()
                            .expect("bluetooth_devices lock poisoned");
                        if let Some(existing) = guard.get_mut(selected_index) {
                            existing.paired = false;
                            existing.trusted = false;
                            existing.connected = false;
                        }
                    }
                    let guard = bluetooth_devices
                        .lock()
                        .expect("bluetooth_devices lock poisoned");
                    sync_bluetooth_selection(&app, &guard, selected_index.min(guard.len().saturating_sub(1)));
                    if app.get_paired_bluetooth_address() == device.address {
                        app.set_paired_bluetooth_address("".into());
                    }
                    app.set_bluetooth_pairing_status(
                        format!("Unpaired {}.", device.name).into(),
                    );
                }
                Err(error) => {
                    app.set_bluetooth_pairing_status(
                        format!("Failed to unpair {}: {error}", device.name).into(),
                    );
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        app.on_cancel_bluetooth_pairing(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            stop_bluetooth_pairing_light_blink();
            app.set_bluetooth_pairing_status("".into());
            app.set_paired_bluetooth_address("".into());
            app.set_bluetooth_pairing_intro(false);
            app.set_bluetooth_scan_running(false);
            app.set_bluetooth_link_light_off_visible(false);
            app.set_bluetooth_device_action(BLUETOOTH_ACTION_PRIMARY_BUTTON);
            app.set_screen(5);
        });
    }

    {
        let app_weak = app.as_weak();
        let setup_state = Arc::clone(&setup_state);
        app.on_controller_device_select_prev(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            {
                let mut guard = setup_state.lock().expect("setup_state lock poisoned");
                let Some(state) = guard.as_mut() else {
                    return;
                };
                state.select_previous_device();
            }

            refresh_controller_setup_view(&app, &setup_state);
        });
    }

    {
        let app_weak = app.as_weak();
        let setup_state = Arc::clone(&setup_state);
        app.on_controller_device_select_next(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            {
                let mut guard = setup_state.lock().expect("setup_state lock poisoned");
                let Some(state) = guard.as_mut() else {
                    return;
                };
                state.select_next_device();
            }

            refresh_controller_setup_view(&app, &setup_state);
        });
    }

    {
        let app_weak = app.as_weak();
        let setup_state = Arc::clone(&setup_state);
        app.on_activate_controller_device_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };

            let selected_device_name = {
                let mut guard = setup_state.lock().expect("setup_state lock poisoned");
                let Some(state) = guard.as_mut() else {
                    return;
                };
                state
                    .confirm_selected_device()
                    .map(|device| device.name)
            };

            let Some(selected_device_name) = selected_device_name else {
                app.set_controller_config_last_input("No controller device selected.".into());
                refresh_controller_setup_view(&app, &setup_state);
                return;
            };

            app.set_controller_device_selection(false);
            app.set_controller_capture_armed(false);
            app.set_controller_config_last_input(
                format!("Selected device: {selected_device_name}. Waiting for button presses...")
                    .into(),
            );
            refresh_controller_setup_view(&app, &setup_state);
        });
    }

    {
        let app_weak = app.as_weak();
        let setup_state = Arc::clone(&setup_state);
        app.on_cancel_controller_setup(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let return_to_manage = {
                let guard = setup_state.lock().expect("setup_state lock poisoned");
                guard.as_ref().is_some_and(|state| !state.is_device_selection())
            };
            let _ = send_runtime_mapper_toggle_request(&supervisor_socket_path(), true);
            app.set_controller_config_instruction("Controller setup cancelled.".into());
            app.set_controller_config_last_input("Cancelled".into());
            app.set_controller_device_selection(false);
            *setup_state.lock().expect("setup_state lock poisoned") = None;
            app.set_screen(if return_to_manage { 4 } else { 5 });
            app.set_focus_region(1);
        });
    }

    {
        let app_weak = app.as_weak();
        let setup_state = Arc::clone(&setup_state);
        let selected_index = Arc::clone(&selected_index);
        app.on_activate_controller_final_selection(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let confirm = app.get_controller_final_selection() == 0;
            finalize_controller_setup(&app, &setup_state, &selected_index, confirm);
        });
    }

    app.run()
}
