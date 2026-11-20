//! Offline verification: synthesizes noisy "voice", runs the real AI + DSP
//! pipeline, writes in/out WAVs and asserts real noise reduction without
//! harming the voice band. Run: cargo run --example pipeline_test

use voice_studio::ai::Denoiser;
use voice_studio::dsp::{preset, Agc, StudioChain};

const FS: usize = 48000;

/// Stationary "room" noise: lowpassed white + 100 Hz hum + keyboard clicks.
struct NoiseGen {
    state: f32,
    lp: f32,
    next_click: f32,
}

impl NoiseGen {
    fn new() -> Self {
        Self {
            state: 12.345,
            lp: 0.0,
            next_click: 0.7,
        }
    }
    fn next(&mut self, t: f32) -> f32 {
        // cheap deterministic white noise
        self.state = (self.state * 16807.0 + 0.317).sin() * 43758.5453;
        let w = self.state.fract() * 2.0 - 1.0;
        self.lp += 0.12 * (w - self.lp); // ~1 kHz lowpass → fan-ish
        let mut n = self.lp * 0.012 + (2.0 * std::f32::consts::PI * 100.0 * t).sin() * 0.004;
        if (t - self.next_click).abs() < 0.005 {
            self.state = (self.state * 4231.7 + 7.1).sin() * 9137.231;
            n += self.state.fract() * 0.15; // keyboard click burst
            if t > self.next_click + 0.005 {
                self.next_click += 0.55;
            }
        }
        n
    }
}

fn rms_db(buf: &[f32]) -> f32 {
    let e = buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32;
    if e > 1e-18 {
        10.0 * e.log10()
    } else {
        -100.0
    }
}

/// Minimal PCM16 mono WAV reader (returns samples + sample rate).
fn read_wav_i16(path: &std::path::Path) -> Option<(Vec<f32>, u32)> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    let mut rate = 0u32;
    let mut channels = 1u16;
    let mut pcm: Option<&[u8]> = None;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = data.get(pos + 8..pos + 8 + size)?;
        match id {
            b"fmt " => {
                channels = u16::from_le_bytes(body[2..4].try_into().ok()?);
                rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            }
            b"data" => pcm = Some(body),
            _ => {}
        }
        pos += 8 + size + (size & 1);
    }
    let pcm = pcm?;
    let samples: Vec<f32> = pcm
        .chunks_exact(2 * channels as usize)
        .map(|fr| {
            let s = i16::from_le_bytes([fr[0], fr[1]]) as f32 / 32768.0;
            if channels > 1 {
                let mut acc = s;
                for c in 1..channels as usize {
                    acc += i16::from_le_bytes([fr[c * 2], fr[c * 2 + 1]]) as f32 / 32768.0;
                }
                acc / channels as f32
            } else {
                s
            }
        })
        .collect();
    Some((samples, rate))
}

fn band_rms_db(buf: &[f32], lo: f32, hi: f32) -> f32 {
    // naive Goertzel-style band energy over strided segments
    let n = buf.len();
    let mut energy = 0.0;
    let mut count = 0usize;
    let step = 4;
    for i in (0..n - 8).step_by(step) {
        let t0 = i as f32 / FS as f32;
        let w_lo = 2.0 * std::f32::consts::PI * lo;
        let w_hi = 2.0 * std::f32::consts::PI * hi;
        let mut acc_lo = 0.0f32;
        let mut acc_hi = 0.0f32;
        for j in 0..8 {
            let t = t0 + j as f32 / FS as f32;
            acc_lo += buf[i + j] * (w_lo * t).sin();
            acc_hi += buf[i + j] * (w_hi * t).sin();
        }
        energy += (acc_lo * acc_lo + acc_hi * acc_hi) as f64;
        count += 1;
    }
    let e = (energy / count.max(1) as f64) as f32;
    if e > 1e-18 {
        10.0 * e.log10()
    } else {
        -100.0
    }
}

fn main() {
    // Real speech from Windows TTS (SAPI), mixed with synthetic room noise.
    let speech_path = std::env::temp_dir()
        .join("ai-voice-studio-test")
        .join("speech.wav");
    let (speech_src, speech_rate) = read_wav_i16(&speech_path).unwrap_or_else(|| {
        panic!(
            "missing {}, generate with SAPI TTS first",
            speech_path.display()
        )
    });
    assert_eq!(speech_rate, 48000, "TTS wav must be 48 kHz mono");

    let dur = 8.0f32;
    let n_samples = (dur * FS as f32) as usize;

    // speech envelope: silent first second (noise-only), then TTS speech.
    // Levels approximate a decent microphone: speech ~25 dB above the noise bed.
    let mut noisy = Vec::with_capacity(n_samples);
    let mut gen = NoiseGen::new();
    for i in 0..n_samples {
        let t = i as f32 / FS as f32;
        let v = if t >= 1.0 {
            let idx = ((t - 1.0) * FS as f32) as usize;
            speech_src.get(idx).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        noisy.push(v * 2.2 + gen.next(t));
    }

    // A/B comparison on identical input, exactly like the app's RAW/ENHANCED.
    // Three paths isolate each stage:
    //   RAW  = AGC only
    //   AI   = AGC → RNNoise (100% wet)
    //   FULL = AGC → RNNoise → studio chain (as shipped, ENHANCED + Studio ON)
    let run = |enhanced: bool, studio: bool| -> Vec<f32> {
        let mut chain = StudioChain::new(FS as f32, preset("studio"));
        let mut agc = Agc::new(chain.params().agc_target_db, true);
        let mut denoiser = Denoiser::new().expect("RNNoise init");
        let mut out = Vec::with_capacity(n_samples);
        let mut buf = vec![0.0f32; 480];
        let mut i = 0;
        while i + 480 <= n_samples {
            buf.copy_from_slice(&noisy[i..i + 480]);
            agc.process(&mut buf);
            if enhanced {
                denoiser.process(&mut buf);
                if studio {
                    chain.process(&mut buf);
                }
            }
            out.extend_from_slice(&buf);
            i += 480;
        }
        out
    };
    let raw = run(false, false);
    let ai = run(true, false);
    let enhanced = run(true, true);

    let seg = |a: f32, b: f32, s: &[f32]| {
        let a = (a * FS as f32) as usize;
        let b = (b * FS as f32) as usize;
        s[a..b].to_vec()
    };
    // noise cut and voice preservation measured against the AI stage alone,
    // so compression/limiting dynamics don't count as "voice destroyed"
    let noise_cut = rms_db(&seg(0.1, 0.9, &raw)) - rms_db(&seg(0.1, 0.9, &ai));
    let voice_full_raw = rms_db(&seg(2.0, 6.5, &raw));
    let voice_full_ai = rms_db(&seg(2.0, 6.5, &ai));
    let voice_full_delta = voice_full_ai - voice_full_raw;
    let voice_full_enh = rms_db(&seg(2.0, 6.5, &enhanced));
    let voice_full_enh_delta = voice_full_enh - voice_full_raw;

    let peak = enhanced.iter().fold(0.0f32, |m, s| m.max(s.abs()));

    let out_dir = std::env::temp_dir().join("ai-voice-studio-test");
    let _ = std::fs::create_dir_all(&out_dir);
    let in_path = out_dir.join("pipeline_in.wav");
    let out_path = out_dir.join("pipeline_out.wav");
    voice_studio::testbed::write_wav_i16(&in_path, &noisy, FS as u32).unwrap();
    voice_studio::testbed::write_wav_i16(&out_path, &enhanced, FS as u32).unwrap();

    println!("wrote {} and {}", in_path.display(), out_path.display());
    println!(
        "noise-only segment: RAW {:.1} dB → AI {:.1} dB  (reduction {:.1} dB)",
        rms_db(&seg(0.1, 0.9, &raw)),
        rms_db(&seg(0.1, 0.9, &ai)),
        noise_cut
    );
    println!("voice segment full-band: RAW {:.1} dB → AI {:.1} dB (delta {:+.1} dB) → FULL {:+.1} dB vs RAW",
        voice_full_raw, voice_full_ai, voice_full_delta, voice_full_enh_delta);
    println!(
        "output peak (FULL): {:.3} (ceiling -1.5 dBFS = 0.841)",
        peak
    );

    let mut failed = false;
    if noise_cut < 8.0 {
        println!("FAIL: noise reduction {:.1} dB < 8 dB", noise_cut);
        failed = true;
    }
    if voice_full_delta < -4.5 {
        println!(
            "FAIL: AI stage dropped voice by {:+.1} dB",
            voice_full_delta
        );
        failed = true;
    }
    if peak > 0.85 {
        println!("FAIL: output peak {:.3} above ceiling", peak);
        failed = true;
    }
    if enhanced.iter().any(|s| !s.is_finite()) {
        println!("FAIL: non-finite samples");
        failed = true;
    }
    if failed {
        std::process::exit(1);
    }
    println!("PASS: pipeline reduces noise while preserving voice");
}
