//! Voice Studio — Tauri application layer: commands, events, tray,
//! settings, autostart, logging.

pub mod ai;
pub mod dsp;
pub mod engine;
mod eye_tracker;
pub mod settings;
pub mod testbed;
pub mod virtual_mic;
pub mod vsmic;

use engine::{Controls, DeviceLists, DeviceSelection, Engine, Snapshot};
use serde_json::Value;
use settings::Settings;
use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
use winreg::RegKey;

pub struct AppState {
    pub engine: Arc<Engine>,
    pub settings: Mutex<Settings>,
}

// ---------------------------------------------------------------------------
// Logging (file + stderr; never logs audio content)
// ---------------------------------------------------------------------------

struct FileLogger(Mutex<Option<fs::File>>);
static LOG_SEQ: AtomicU64 = AtomicU64::new(0);

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let line = format!("[{ms}] [{}] {}", record.level(), record.args());
        eprintln!("{line}");
        if let Ok(mut guard) = self.0.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = writeln!(f, "{line}");
            }
        }
    }
    fn flush(&self) {}
}

fn init_logging() {
    let dir = settings::config_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("voice-studio.log");
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > 2_000_000 {
            let _ = fs::rename(&path, dir.join("voice-studio.old.log"));
        }
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    let _ = log::set_boxed_logger(Box::new(FileLogger(Mutex::new(file))));
    log::set_max_level(log::LevelFilter::Info);
}

// ---------------------------------------------------------------------------
// Autostart (HKCU Run key)
// ---------------------------------------------------------------------------

fn set_autostart(enable: bool, launch_minimized: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            KEY_SET_VALUE,
        )
        .map_err(|e| e.to_string())?;
    let _ = key.delete_value("AI Voice Studio");
    if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let args = if launch_minimized { " --minimized" } else { "" };
        key.set_value("Voice Studio", &format!("\"{}\"{args}", exe.display()))
            .map_err(|e| e.to_string())?;
    } else {
        let _ = key.delete_value("Voice Studio");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// App bootstrap
// ---------------------------------------------------------------------------

pub fn run() {
    let mut settings = settings::load();
    init_logging();
    log::info!("Voice Studio starting");

    // glue the processed stream to whatever virtual microphone backend exists
    // (own driver preferred; VB-CABLE development fallback) — no user routing
    match virtual_mic::auto_route_target(&settings.virtual_output) {
        Some(new) => {
            settings.virtual_output = if new.is_empty() { None } else { Some(new) };
            settings::save(&settings);
            log::info!(
                "virtual microphone backend auto-configured: {:?}",
                settings.virtual_output
            );
        }
        None => log::info!("no virtual microphone backend detected yet (setup available in UI)"),
    }

    let controls = Arc::new(Controls::new());
    controls
        .mode_enhanced
        .store(settings.mode_enhanced, Ordering::Relaxed);
    controls
        .blend_permille
        .store(settings.noise_strength.min(100) * 10, Ordering::Relaxed);
    controls
        .studio_on
        .store(settings.studio_enhance, Ordering::Relaxed);
    controls
        .monitor_on
        .store(settings.monitor_enabled, Ordering::Relaxed);
    controls
        .monitor_vol_permille
        .store(settings.monitor_volume.min(100) * 10, Ordering::Relaxed);
    controls
        .auto_gain
        .store(settings.auto_gain, Ordering::Relaxed);

    let sel = DeviceSelection {
        input: settings.input_device.clone(),
        monitor: settings.monitor_device.clone(),
        virtual_out: settings.virtual_output.clone(),
        buffer_frames: settings::quality_buffer(&settings.quality),
    };
    let engine = Engine::new(sel, controls);
    engine.set_preset_params(dsp::preset(&settings.preset));
    *engine.equalizers.lock().unwrap() = (settings.mic_eq.clone(), settings.speaker_eq.clone());

    let state = AppState {
        engine,
        settings: Mutex::new(settings),
    };

    tauri::Builder::default()
        .manage(state)
        .manage(eye_tracker::EyeTracker::default())
        .invoke_handler(tauri::generate_handler![
            get_devices,
            eye_tracker::eye_tracker_status,
            eye_tracker::eye_tracker_start,
            eye_tracker::eye_tracker_command,
            eye_tracker::eye_tracker_stop,
            get_ui_state,
            set_input_device,
            set_monitor_device,
            set_virtual_output,
            set_mode,
            set_noise_strength,
            set_studio,
            set_preset,
            set_equalizer,
            set_monitor_enabled,
            set_monitor_volume,
            set_auto_gain,
            set_setting,
            virtual_mic_status,
            virtual_mic_setup,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || event_pump(handle));

            // tray icon
            let show = MenuItem::with_id(app, "show", "Show Voice Studio", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Voice Studio")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            app.manage(tray);

            // launch minimized (setting or --minimized flag from autostart)
            let minimized = app
                .state::<AppState>()
                .settings
                .lock()
                .unwrap()
                .launch_minimized
                || std::env::args().any(|a| a == "--minimized");
            if minimized {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            log::info!("app setup complete");
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let minimize = window
                    .app_handle()
                    .state::<AppState>()
                    .settings
                    .lock()
                    .unwrap()
                    .minimize_to_tray;
                if minimize {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Voice Studio")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                app.state::<eye_tracker::EyeTracker>().stop();
            }
        });
}

fn event_pump(handle: AppHandle) {
    let state = handle.state::<AppState>();
    loop {
        if let Some(snap) = state.engine.snapshot() {
            let _ = handle.emit("engine-snapshot", &snap);
        }
        let mut devices_changed = false;
        while state.engine.poll_event().is_some() {
            devices_changed = true;
        }
        if devices_changed {
            log::info!("device list changed");
            let _ = handle.emit("devices-changed", Engine::list_devices());
            // keep the virtual microphone routing glued to whatever backend
            // is available (own driver preferred, VB-CABLE fallback)
            auto_route_apply(&state);
        }
        std::thread::sleep(Duration::from_millis(33));
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    pub snapshot: Option<Snapshot>,
    pub settings: Settings,
    pub selection: DeviceSelection,
    pub devices: DeviceLists,
}

#[tauri::command]
fn get_devices() -> DeviceLists {
    Engine::list_devices()
}

#[tauri::command]
fn get_ui_state(state: State<AppState>) -> UiState {
    let settings = state.settings.lock().unwrap().clone();
    UiState {
        snapshot: state.engine.snapshot(),
        settings,
        selection: state.engine.current_selection(),
        devices: Engine::list_devices(),
    }
}

fn sync_selection(state: &AppState) {
    let s = state.settings.lock().unwrap();
    let sel = DeviceSelection {
        input: s.input_device.clone(),
        monitor: s.monitor_device.clone(),
        virtual_out: s.virtual_output.clone(),
        buffer_frames: settings::quality_buffer(&s.quality),
    };
    drop(s);
    state.engine.apply_devices(sel);
}

fn persist(state: &AppState) {
    let s = state.settings.lock().unwrap().clone();
    settings::save(&s);
}

#[tauri::command]
fn set_input_device(state: State<AppState>, name: Option<String>) {
    state.settings.lock().unwrap().input_device = name;
    persist(&state);
    sync_selection(&state);
}

#[tauri::command]
fn set_monitor_device(state: State<AppState>, name: Option<String>) {
    state.settings.lock().unwrap().monitor_device = name;
    persist(&state);
    sync_selection(&state);
}

#[tauri::command]
fn set_virtual_output(state: State<AppState>, name: Option<String>) {
    state.settings.lock().unwrap().virtual_output = name;
    persist(&state);
    sync_selection(&state);
}

#[tauri::command]
fn set_mode(state: State<AppState>, enhanced: bool) {
    state
        .engine
        .controls
        .mode_enhanced
        .store(enhanced, Ordering::Relaxed);
    state.settings.lock().unwrap().mode_enhanced = enhanced;
    persist(&state);
}

#[tauri::command]
fn set_noise_strength(state: State<AppState>, value: u32) {
    let v = value.min(100);
    state
        .engine
        .controls
        .blend_permille
        .store(v * 10, Ordering::Relaxed);
    state.settings.lock().unwrap().noise_strength = v;
    persist(&state);
}

#[tauri::command]
fn set_studio(state: State<AppState>, on: bool) {
    state.engine.controls.studio_on.store(on, Ordering::Relaxed);
    state.settings.lock().unwrap().studio_enhance = on;
    persist(&state);
}

#[tauri::command]
fn set_preset(state: State<AppState>, name: String) -> Result<(), String> {
    if ![
        "natural",
        "studio",
        "gaming",
        "broadcast",
        "podcast",
        "warm",
        "clarity",
        "meeting",
    ]
    .contains(&name.as_str())
    {
        return Err("Unknown voice preset".into());
    }
    state.engine.set_preset_params(dsp::preset(&name));
    state.settings.lock().unwrap().preset = name;
    persist(&state);
    Ok(())
}

#[tauri::command]
fn set_monitor_enabled(state: State<AppState>, on: bool) {
    state
        .engine
        .controls
        .monitor_on
        .store(on, Ordering::Relaxed);
    state.settings.lock().unwrap().monitor_enabled = on;
    persist(&state);
}

#[tauri::command]
fn set_monitor_volume(state: State<AppState>, value: u32) {
    let v = value.min(100);
    state
        .engine
        .controls
        .monitor_vol_permille
        .store(v * 10, Ordering::Relaxed);
    state.settings.lock().unwrap().monitor_volume = v;
    persist(&state);
}

#[tauri::command]
fn set_auto_gain(state: State<AppState>, on: bool) {
    state.engine.controls.auto_gain.store(on, Ordering::Relaxed);
    state.settings.lock().unwrap().auto_gain = on;
    persist(&state);
}

#[tauri::command]
fn set_setting(state: State<AppState>, key: String, value: Value) {
    let mut restart = false;
    let mut autostart = None;
    {
        let mut s = state.settings.lock().unwrap();
        match key.as_str() {
            "startWithWindows" => {
                if let Some(b) = value.as_bool() {
                    s.start_with_windows = b;
                    autostart = Some(b);
                }
            }
            "minimizeToTray" => {
                if let Some(b) = value.as_bool() {
                    s.minimize_to_tray = b;
                }
            }
            "launchMinimized" => {
                if let Some(b) = value.as_bool() {
                    s.launch_minimized = b;
                    autostart = Some(s.start_with_windows);
                }
            }
            "quality" => {
                if let Some(q) = value.as_str() {
                    s.quality = q.to_string();
                    restart = true;
                }
            }
            _ => {}
        }
    }
    persist(&state);
    if restart {
        sync_selection(&state);
    }
    if let Some(enable) = autostart {
        let minimized = state.settings.lock().unwrap().launch_minimized;
        match set_autostart(enable, minimized) {
            Ok(()) => log::info!("autostart set to {enable}"),
            Err(e) => log::error!("autostart failed: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Virtual microphone: backend abstraction (own driver preferred, VB-CABLE
// temporary development fallback) + fully automated routing and setup.
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualMicStatus {
    /// "ready" | "setup-required" | "installing"
    pub status: String,
    /// "own-driver" | "vb-cable" | null
    pub backend: Option<String>,
    /// Microphone name the user should pick in Discord / FiveM / OBS.
    pub select_in_apps: Option<String>,
    pub message: String,
}

/// Applies the current backend's routing decision to settings + engine.
fn auto_route_apply(state: &AppState) -> bool {
    let current = state.settings.lock().unwrap().virtual_output.clone();
    match virtual_mic::auto_route_target(&current) {
        None => false,
        Some(new) => {
            let new = if new.is_empty() { None } else { Some(new) };
            {
                let mut s = state.settings.lock().unwrap();
                s.virtual_output = new.clone();
            }
            persist(state);
            let route_settings = state.settings.lock().unwrap().clone();
            let sel = DeviceSelection {
                input: route_settings.input_device.clone(),
                monitor: route_settings.monitor_device.clone(),
                virtual_out: new,
                buffer_frames: settings::quality_buffer(&route_settings.quality),
            };
            state.engine.apply_devices(sel);
            log::info!("virtual microphone auto-routed");
            true
        }
    }
}

#[tauri::command]
fn virtual_mic_status(state: State<AppState>) -> VirtualMicStatus {
    let info = virtual_mic::detect();
    let backend_name = match info.backend {
        virtual_mic::Backend::OwnDriver => Some("own-driver".to_string()),
        virtual_mic::Backend::VbCable => Some("vb-cable".to_string()),
        virtual_mic::Backend::None => None,
    };
    let installing = virtual_mic::install_state() == 1;
    let routed = {
        let s = state.settings.lock().unwrap();
        s.virtual_output
            .as_deref()
            .map(|v| v.contains("CABLE Input"))
            .unwrap_or(false)
    };
    let (status, message) = match info.backend {
        virtual_mic::Backend::OwnDriver => (
            "ready",
            "Voice Studio Microphone active — select it in Discord / FiveM / OBS.".to_string(),
        ),
        virtual_mic::Backend::VbCable if routed => (
            "ready",
            "Ready — temporary development virtual microphone in use.".to_string(),
        ),
        virtual_mic::Backend::VbCable => (
            "setup-required",
            "Virtual device detected — finishing routing…".to_string(),
        ),
        virtual_mic::Backend::None if installing => (
            "installing",
            "Installing the temporary virtual microphone…".to_string(),
        ),
        virtual_mic::Backend::None => (
            "setup-required",
            "Setup required — one click installs the temporary virtual microphone.".to_string(),
        ),
    };
    VirtualMicStatus {
        status: status.to_string(),
        backend: backend_name,
        select_in_apps: info.capture_name,
        message,
    }
}

#[tauri::command]
fn virtual_mic_setup(app: AppHandle) {
    if virtual_mic::install_state() == 1 {
        return;
    }
    std::thread::spawn(move || run_vbcable_setup(app));
}

const VBCABLE_URL: &str = "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack43.zip";

fn ps(script: &str) -> bool {
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn vm_emit(app: &AppHandle, stage: &str, message: &str) {
    log::info!("vm-setup [{stage}] {message}");
    let _ = app.emit(
        "vm-setup",
        serde_json::json!({ "stage": stage, "message": message }),
    );
}

/// Fully automated VB-CABLE bring-up: reuse a local driver pack when present,
/// otherwise download it; run the vendor installer elevated (the single
/// Windows-permission gate); wait for the endpoints; auto-route and remember.
fn run_vbcable_setup(app: AppHandle) {
    virtual_mic::set_install_state(1);
    let finish = |app: &AppHandle, msg: &str| {
        virtual_mic::set_install_state(2);
        vm_emit(app, "done", msg);
    };

    if virtual_mic::detect().backend == virtual_mic::Backend::VbCable {
        auto_route_apply(&app.state::<AppState>());
        let _ = app.emit("devices-changed", Engine::list_devices());
        finish(&app, "Ready — temporary virtual microphone connected.");
        return;
    }

    // prefer a driver pack already on disk (dev machine) over downloading
    let mut candidates = vec![std::env::temp_dir().join("vbcable")];
    candidates.push(std::env::temp_dir().join("aivs-vmcable"));
    let mut zip: Option<std::path::PathBuf> = None;
    let mut setup_exe: Option<std::path::PathBuf> = None;
    for dir in &candidates {
        let exe = dir.join("VBCABLE_Setup_x64.exe");
        if exe.exists() {
            setup_exe = Some(exe);
            zip = Some(dir.join("vbcable.zip"));
            break;
        }
    }

    if setup_exe.is_none() {
        vm_emit(
            &app,
            "download",
            "Downloading the temporary virtual microphone driver…",
        );
        let zip_path = std::env::temp_dir().join("aivs-vmcable.zip");
        let _ = std::fs::remove_file(&zip_path);
        if !ps(&format!(
            "Invoke-WebRequest -Uri '{VBCABLE_URL}' -OutFile '{}' -UseBasicParsing",
            zip_path.display()
        )) {
            virtual_mic::set_install_state(3);
            vm_emit(
                &app,
                "error",
                "Could not download the driver package (check internet).",
            );
            return;
        }
        let dir = std::env::temp_dir().join("aivs-vmcable");
        let _ = std::fs::remove_dir_all(&dir);
        vm_emit(&app, "extract", "Extracting…");
        if !ps(&format!(
            "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
            zip_path.display(),
            dir.display()
        )) {
            virtual_mic::set_install_state(3);
            vm_emit(&app, "error", "Could not extract the driver package.");
            return;
        }
        setup_exe = Some(dir.join("VBCABLE_Setup_x64.exe"));
        zip = Some(dir.join("vbcable.zip"));
    }
    let _ = zip;

    let setup = setup_exe.unwrap();
    if !setup.exists() {
        virtual_mic::set_install_state(3);
        vm_emit(&app, "error", "Driver package is missing its installer.");
        return;
    }

    vm_emit(
        &app,
        "elevate",
        "Approve the UAC prompt, then click Install Driver in the setup window.",
    );
    if !ps(&format!(
        "Start-Process -FilePath '{}' -Verb RunAs -WorkingDirectory '{}'",
        setup.display(),
        setup.parent().unwrap().display()
    )) {
        virtual_mic::set_install_state(3);
        vm_emit(&app, "error", "The elevated install was cancelled.");
        return;
    }

    vm_emit(
        &app,
        "waiting",
        "Waiting for the virtual microphone to appear…",
    );
    for _ in 0..120 {
        std::thread::sleep(Duration::from_secs(2));
        if virtual_mic::detect().backend == virtual_mic::Backend::VbCable {
            auto_route_apply(&app.state::<AppState>());
            let _ = app.emit("devices-changed", Engine::list_devices());
            finish(&app, "Ready — temporary virtual microphone connected.");
            return;
        }
    }
    virtual_mic::set_install_state(3);
    vm_emit(
        &app,
        "error",
        "Timed out waiting for the driver — try Setup again.",
    );
}

#[tauri::command]
fn set_equalizer(
    state: State<AppState>,
    target: String,
    config: settings::EqConfig,
) -> Result<(), String> {
    if target != "mic" && target != "speaker" {
        return Err("Unknown equalizer".into());
    }
    if !config.preamp.is_finite()
        || config.preamp.abs() > 12.0
        || config
            .gains
            .iter()
            .any(|g| !g.is_finite() || g.abs() > 12.0)
    {
        return Err("EQ gains must be between -12 and +12 dB".into());
    }
    {
        let mut s = state.settings.lock().unwrap();
        if target == "mic" {
            s.mic_eq = config;
        } else {
            s.speaker_eq = config;
        }
        *state.engine.equalizers.lock().unwrap() = (s.mic_eq.clone(), s.speaker_eq.clone());
    }
    persist(&state);
    Ok(())
}
