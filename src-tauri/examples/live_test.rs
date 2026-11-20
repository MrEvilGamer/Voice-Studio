//! Live verification: enumerates WASAPI devices and captures 3 s of real
//! microphone audio through the full pipeline into WAV files.
//! Exit 0 = live audio verified · 2 = no input device (skip, not failure).

fn main() {
    let devices = voice_studio::engine::Engine::list_devices();
    println!(
        "input devices ({}): {}",
        devices.inputs.len(),
        devices
            .inputs
            .iter()
            .map(|d| if d.is_default {
                format!("[default] {}", d.name)
            } else {
                d.name.clone()
            })
            .collect::<Vec<_>>()
            .join(" | ")
    );
    println!(
        "output devices ({}): {}",
        devices.outputs.len(),
        devices
            .outputs
            .iter()
            .map(|d| if d.is_default {
                format!("[default] {}", d.name)
            } else {
                d.name.clone()
            })
            .collect::<Vec<_>>()
            .join(" | ")
    );

    if devices.inputs.is_empty() {
        println!("SKIP: no input devices on this machine");
        std::process::exit(2);
    }

    let stem = std::env::temp_dir()
        .join("ai-voice-studio-test")
        .join("live");
    let device = std::env::args().nth(1);
    match voice_studio::testbed::capture_to_wav(device.as_deref(), 3.0, &stem, true) {
        Ok(stats) => {
            println!("LIVE CAPTURE OK: {stats}");
        }
        Err(e) => {
            println!("LIVE CAPTURE FAILED: {e}");
            std::process::exit(1);
        }
    }
}
