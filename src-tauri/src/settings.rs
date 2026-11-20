//! Local-first settings persisted to %APPDATA%\AI Voice Studio\settings.json.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub input_device: Option<String>,
    pub monitor_device: Option<String>,
    pub virtual_output: Option<String>,
    pub noise_strength: u32,
    pub studio_enhance: bool,
    pub preset: String,
    pub monitor_enabled: bool,
    pub monitor_volume: u32,
    pub auto_gain: bool,
    pub mode_enhanced: bool,
    pub start_with_windows: bool,
    pub minimize_to_tray: bool,
    pub launch_minimized: bool,
    pub quality: String,
    pub mic_eq: EqConfig,
    pub speaker_eq: EqConfig,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input_device: None,
            monitor_device: None,
            virtual_output: None,
            noise_strength: 75,
            studio_enhance: true,
            preset: "studio".into(),
            monitor_enabled: false,
            monitor_volume: 80,
            auto_gain: true,
            mode_enhanced: true,
            start_with_windows: false,
            minimize_to_tray: true,
            launch_minimized: false,
            quality: "balanced".into(),
            mic_eq: EqConfig::default(),
            speaker_eq: EqConfig::default(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("Voice Studio")
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    let path = settings_path();
    let path = if path.exists() {
        path
    } else {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
            .join("AI Voice Studio/settings.json")
    };
    match fs::read_to_string(&path) {
        Ok(txt) => match serde_json::from_str(&txt) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("settings parse error: {e}");
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

pub fn save(s: &Settings) {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = fs::write(settings_path(), json);
    }
}

/// Output stream buffer frames for the "processing quality" setting.
pub fn quality_buffer(quality: &str) -> u32 {
    match quality {
        "low" => 480,   // ~10 ms — lowest latency
        "safe" => 1920, // ~40 ms — maximum stability
        _ => 960,       // balanced ~20 ms
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct EqConfig {
    pub enabled: bool,
    pub gains: [f32; 10],
    pub preamp: f32,
}
impl Default for EqConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            gains: [0.0; 10],
            preamp: 0.0,
        }
    }
}
