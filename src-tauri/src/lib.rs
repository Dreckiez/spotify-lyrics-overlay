mod lyrics;
mod media;

use media::PlaybackRuntime;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Builder as ShortcutBuilder, GlobalShortcutExt, ShortcutState};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct AppConfig {
    text_color: String,
    active_color: String,
    background_opacity: u8,
    font_family: String,
    font_size: u8,
    alignment: String,
    offset_ms: i32,
    app_scale: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            text_color: "#e9edfa".into(),
            active_color: "#68f1ce".into(),
            background_opacity: 24,
            font_family: "Segoe UI".into(),
            font_size: 22,
            alignment: "left".into(),
            offset_ms: 0,
            app_scale: 100,
        }
    }
}

impl AppConfig {
    fn validate(self) -> Result<Self, String> {
        if !is_hex_color(&self.text_color) || !is_hex_color(&self.active_color) {
            return Err("Màu cần có định dạng #RRGGBB".into());
        }
        if !(12..=36).contains(&self.font_size) {
            return Err("Cỡ chữ cần nằm trong khoảng 12–36 px".into());
        }
        if !["Segoe UI", "Arial", "Verdana", "Georgia", "monospace"]
            .contains(&self.font_family.as_str())
        {
            return Err("Phông chữ không được hỗ trợ".into());
        }
        if !["left", "center", "right"].contains(&self.alignment.as_str()) {
            return Err("Căn lề không hợp lệ".into());
        }
        if !(-2000..=2000).contains(&self.offset_ms) {
            return Err("Độ trễ cần nằm trong khoảng -2000 đến 2000 ms".into());
        }
        if !(50..=200).contains(&self.app_scale) {
            return Err("Tỷ lệ thu phóng cần nằm trong khoảng 50–200%".into());
        }
        Ok(self)
    }
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

struct AppState {
    config: Mutex<AppConfig>,
    config_path: PathBuf,
    click_through: AtomicBool,
    playback: Arc<PlaybackRuntime>,
}

#[derive(Clone, Debug, Serialize)]
struct LyricTick {
    title: Option<String>,
    artist: Option<String>,
    source: Option<String>,
    position_ms: u64,
    duration_ms: Option<u64>,
    is_playing: bool,
    active_index: Option<usize>,
    lines: Vec<lyrics::LyricLine>,
    lyrics_state: String,
}

impl LyricTick {
    fn empty() -> Self {
        Self {
            title: None,
            artist: None,
            source: None,
            position_ms: 0,
            duration_ms: None,
            is_playing: false,
            active_index: None,
            lines: Vec::new(),
            lyrics_state: "idle".into(),
        }
    }
}

#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    state
        .config
        .lock()
        .map(|config| config.clone())
        .map_err(|_| "Không đọc được cài đặt do lỗi đồng bộ nội bộ".into())
}

#[tauri::command]
fn save_config(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    config: AppConfig,
) -> Result<AppConfig, String> {
    let config = config.validate()?;
    let json = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("Không mã hóa được cài đặt: {error}"))?;
    if let Some(parent) = state.config_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Không tạo được thư mục cài đặt: {error}"))?;
    }
    fs::write(&state.config_path, json)
        .map_err(|error| format!("Không ghi được config.json: {error}"))?;

    *state
        .config
        .lock()
        .map_err(|_| "Không lưu được cài đặt do lỗi đồng bộ nội bộ".to_owned())? = config.clone();
    state.playback.set_offset_ms(config.offset_ms as i64);
    let _ = app.emit("app-config", config.clone());
    Ok(config)
}

#[tauri::command]
async fn set_click_through(app: AppHandle, enabled: bool) -> Result<bool, String> {
    apply_click_through(&app, enabled)
}

fn apply_click_through(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Không tìm thấy cửa sổ overlay".to_owned())?;
    window
        .set_ignore_cursor_events(enabled)
        .map_err(|error| format!("Không đổi được chế độ chuột: {error}"))?;
    app.state::<AppState>()
        .click_through
        .store(enabled, Ordering::Relaxed);
    let _ = app.emit("click-through-changed", enabled);
    Ok(enabled)
}

fn schedule_click_through_toggle(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let enabled = !app
            .state::<AppState>()
            .click_through
            .load(Ordering::Relaxed);
        if let Err(error) = apply_click_through(&app, enabled) {
            eprintln!("Could not toggle click-through mode: {error}");
        }
    });
}

fn load_config(path: &PathBuf) -> AppConfig {
    let Ok(json) = fs::read_to_string(path) else {
        return AppConfig::default();
    };
    let config = match serde_json::from_str::<AppConfig>(&json) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Ignoring invalid config.json: {error}");
            return AppConfig::default();
        }
    };
    config.validate().unwrap_or_default()
}

fn create_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(
        app,
        "toggle-click-through",
        "Bật/tắt xuyên chuột",
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "open-settings", "Cài đặt", true, None::<&str>)?;
    let quit = PredefinedMenuItem::quit(app, None)?;
    let menu = Menu::with_items(app, &[&toggle, &settings, &quit])?;

    TrayIconBuilder::new()
        .icon(make_tray_icon())
        .tooltip("Spotify Lyrics Overlay")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle-click-through" => schedule_click_through_toggle(app.clone()),
            "open-settings" => {
                let _ = app.emit("open-settings", ());
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn make_tray_icon() -> Image<'static> {
    const SIZE: usize = 16;
    let mut pixels = vec![0; SIZE * SIZE * 4];
    let mut set_pixel = |x: usize, y: usize| {
        let index = (y * SIZE + x) * 4;
        pixels[index..index + 4].copy_from_slice(&[104, 241, 206, 255]);
    };

    for y in 2..12 {
        set_pixel(10, y);
    }
    for x in 10..15 {
        set_pixel(x, 2);
        set_pixel(x, 3);
    }
    set_pixel(9, 4);
    set_pixel(8, 5);
    for y in 10..14 {
        for x in 1..6 {
            let dx = x as isize - 3;
            let dy = y as isize - 12;
            if dx * dx + dy * dy <= 7 {
                set_pixel(x, y);
            }
        }
    }
    drop(set_pixel);
    Image::new_owned(pixels, SIZE as u32, SIZE as u32)
}

pub fn run() {
    let shortcut_plugin = ShortcutBuilder::new().build();

    tauri::Builder::default()
        .plugin(shortcut_plugin)
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            set_click_through
        ])
        .setup(|app| {
            if let Err(error) = app.global_shortcut().on_shortcut(
                "CommandOrControl+Shift+L",
                |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        schedule_click_through_toggle(app.clone());
                    }
                },
            ) {
                eprintln!("Could not register Ctrl+Shift+L: {error}");
            }

            let config_path = app.path().app_config_dir()?.join("config.json");
            let config = load_config(&config_path);
            let playback = Arc::new(PlaybackRuntime::new(config.offset_ms as i64));
            app.manage(AppState {
                config: Mutex::new(config),
                config_path,
                click_through: AtomicBool::new(false),
                playback: playback.clone(),
            });
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_skip_taskbar(true);
            }
            create_tray(app)?;
            media::start(app.handle().clone(), playback);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Spotify Lyrics Overlay");
}
