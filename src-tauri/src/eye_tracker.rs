use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct EyeTracker {
    child: Mutex<Option<Child>>,
    snapshot: Arc<Mutex<Value>>,
}

impl Default for EyeTracker {
    fn default() -> Self {
        Self { child: Mutex::new(None), snapshot: Arc::new(Mutex::new(json!({"running":false}))) }
    }
}

impl EyeTracker {
    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            if let Some(input) = child.stdin.as_mut() {
                let _ = writeln!(input, "{}", json!({"action":"stop"}));
            }
            // Give Tk a chance to release the camera and save settings.
            for _ in 0..20 {
                if matches!(child.try_wait(), Ok(Some(_))) { break; }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        *self.snapshot.lock().unwrap() = json!({"running":false});
    }
}

impl Drop for EyeTracker { fn drop(&mut self) { self.stop(); } }

#[tauri::command]
pub fn eye_tracker_status(state: State<EyeTracker>) -> Value {
    let mut child = state.child.lock().unwrap();
    if let Some(process) = child.as_mut() {
        if let Ok(Some(code)) = process.try_wait() {
            *state.snapshot.lock().unwrap() = json!({"running":false,"error":format!("Eye tracking stopped ({code}). See eye-tracker.log for details.")});
            *child = None;
        }
    }
    state.snapshot.lock().unwrap().clone()
}

#[tauri::command]
pub fn eye_tracker_start(app: AppHandle, state: State<EyeTracker>) -> Result<(), String> {
    let mut slot = state.child.lock().unwrap();
    if let Some(child) = slot.as_mut() {
        if child.try_wait().map_err(|e| e.to_string())?.is_none() { return Ok(()); }
    }
    let data = crate::settings::config_dir().join("Eye Tracker");
    std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let mut command;
    if cfg!(debug_assertions) {
        command = Command::new("python");
        command.arg("-u").arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tracker/worker.py"));
    } else {
        let path = app.path().resource_dir().map_err(|e| e.to_string())?
            .join("tracker-runtime/eye-worker/eye-worker.exe");
        if !path.exists() { return Err("The eye-tracking runtime is missing. Reinstall Voice Studio.".into()); }
        command = Command::new(path);
    }
    let log = std::fs::OpenOptions::new().create(true).append(true).open(data.join("eye-tracker.log")).map_err(|e| e.to_string())?;
    command.env("VOICE_STUDIO_TRACKER_DATA", &data).env("PYTHONUTF8", "1")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(log));
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|e| format!("Cannot start eye tracking: {e}"))?;
    let output = child.stdout.take().ok_or("Cannot read tracker output")?;
    *state.snapshot.lock().unwrap() = json!({"running":true,"status":"Starting camera…"});
    let snapshot = state.snapshot.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if value.get("type").and_then(Value::as_str) == Some("preview") {
                    let _ = app.emit_to("main", "eye-tracker-preview", &value);
                } else {
                    *snapshot.lock().unwrap() = value;
                }
            }
        }
    });
    *slot = Some(child);
    Ok(())
}

#[tauri::command]
pub fn eye_tracker_command(state: State<EyeTracker>, command: Value) -> Result<(), String> {
    let action = command.get("action").and_then(Value::as_str).unwrap_or("");
    if !["pause", "recenter", "calibrate", "accuracy", "reset", "setting", "preview"].contains(&action) {
        return Err("Unknown eye-tracking action".into());
    }
    let mut child = state.child.lock().unwrap();
    let input = child.as_mut().and_then(|p| p.stdin.as_mut()).ok_or("Start eye tracking first")?;
    writeln!(input, "{command}").and_then(|_| input.flush()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn eye_tracker_stop(state: State<EyeTracker>) { state.stop(); }
