//! Virtual microphone backend abstraction.
//!
//! Preferred backend: our own kernel driver (VoiceStudioMic — see driver/).
//! Temporary development fallback: VB-CABLE, fully automated (detect, install,
//! route, remember) and removed without ceremony once the signed driver ships.
//!
//! The user never configures routing: this module decides where the processed
//! stream goes and keeps that decision applied across launches.

use crate::engine::Engine;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Backend {
    OwnDriver, // VoiceStudioMic loaded; app pushes via vsmic transport
    VbCable,   // temporary fallback: WASAPI render into "CABLE Input"
    None,      // nothing installed yet -> "Setup Required"
}

pub struct BackendInfo {
    pub backend: Backend,
    pub ready: bool,
    /// Playback endpoint the app streams into (VB-CABLE path only).
    pub playback_device: Option<String>,
    /// Endpoint the user selects as a microphone in other apps.
    pub capture_name: Option<String>,
}

static INSTALL_STATE: AtomicU8 = AtomicU8::new(0); // 0 idle, 1 running, 2 done, 3 error

pub fn install_state() -> u8 {
    INSTALL_STATE.load(Ordering::SeqCst)
}

pub fn set_install_state(v: u8) {
    INSTALL_STATE.store(v, Ordering::SeqCst);
}

/// Detects which backend is available right now.
pub fn detect() -> BackendInfo {
    let lists = Engine::list_devices();

    // Preferred: our own kernel driver.
    let own_ready = lists
        .inputs
        .iter()
        .any(|d| d.name.contains("Voice Studio Microphone"))
        && crate::vsmic::is_driver_present();
    if own_ready {
        return BackendInfo {
            backend: Backend::OwnDriver,
            ready: true,
            playback_device: None,
            capture_name: Some("Voice Studio Microphone".into()),
        };
    }

    // Fallback: VB-CABLE.
    let cable_in = lists
        .outputs
        .iter()
        .find(|d| d.name.contains("CABLE Input"))
        .map(|d| d.name.clone());
    if let Some(cin) = cable_in {
        let capture = lists.inputs.iter().any(|d| d.name.contains("CABLE Output"));
        return BackendInfo {
            backend: Backend::VbCable,
            ready: capture,
            playback_device: Some(cin),
            capture_name: Some("CABLE Output (VB-Audio Virtual Cable)".into()),
        };
    }

    BackendInfo {
        backend: Backend::None,
        ready: false,
        playback_device: None,
        capture_name: None,
    }
}

/// Returns the virtual-output device the processed stream should use right
/// now, or None when no WASAPI sink is needed (own driver) / available.
/// An empty-string result is the sentinel for "clear the manual override".
pub fn auto_route_target(current: &Option<String>) -> Option<String> {
    let info = detect();
    match info.backend {
        Backend::OwnDriver => {
            // own driver path bypasses WASAPI sinks entirely
            if current.is_some() {
                Some(String::new())
            } else {
                None
            }
        }
        Backend::VbCable => {
            let target = info.playback_device.clone()?;
            match current {
                Some(c) if c.contains("CABLE Input") => None, // already correct
                _ => Some(target),
            }
        }
        Backend::None => None,
    }
}
