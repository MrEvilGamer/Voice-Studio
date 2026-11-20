//! Headless verification helpers: live microphone capture to WAV, and a
//! minimal WAV writer. Used by the automated pipeline checks — not part of
//! the UI application.

use crate::ai::Denoiser;
use crate::dsp::{preset, Agc, StudioChain};
use crate::engine::{make_resampler, TARGET_RATE};
use rubato::Resampler;
use std::io::Write;
use std::time::{Duration, Instant};

pub fn write_wav_i16(path: &std::path::Path, samples: &[f32], rate: u32) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    let data_len = (samples.len() as u32) * 2;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

pub fn rms_db(buf: &[f32]) -> f32 {
    if buf.is_empty() {
        return -100.0;
    }
    let e = buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32;
    if e > 1e-18 {
        10.0 * e.log10()
    } else {
        -100.0
    }
}

/// Captures `seconds` of live microphone audio and writes:
///   <stem>_raw.wav  (raw capture) and <stem>_enhanced.wav (full pipeline)
/// Returns a short stats line. Fails cleanly when no input device exists.
pub fn capture_to_wav(
    device: Option<&str>,
    seconds: f32,
    stem: &std::path::Path,
    enhanced: bool,
) -> Result<String, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let dev = match device {
        Some(n) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().ok().as_deref() == Some(n))
            .ok_or_else(|| format!("device {n} not found"))?,
        None => host
            .default_input_device()
            .ok_or("no default input device")?,
    };
    let name = dev.name().unwrap_or_default();
    let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
    let rate = cfg.sample_rate().0 as usize;
    let ch = cfg.channels() as usize;

    let (mut tx, mut rx) = rtrb::RingBuffer::<f32>::new(48000 * 8);
    let sc = cfg.config();
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => dev
            .build_input_stream(
                &sc,
                move |d: &[f32], _: &cpal::InputCallbackInfo| {
                    crate::engine::mixdown_push_pub(d, ch, &mut tx);
                },
                |_: cpal::StreamError| {},
                None,
            )
            .map_err(|e| e.to_string())?,
        cpal::SampleFormat::I16 => dev
            .build_input_stream(
                &sc,
                move |d: &[i16], _: &cpal::InputCallbackInfo| {
                    crate::engine::mixdown_push_conv(d, ch, &mut tx, |s| *s as f32 / 32768.0);
                },
                |_: cpal::StreamError| {},
                None,
            )
            .map_err(|e| e.to_string())?,
        _ => return Err("unsupported sample format".into()),
    };
    stream.play().map_err(|e| e.to_string())?;

    let mut raw: Vec<f32> = Vec::new();
    let start = Instant::now();
    while start.elapsed().as_secs_f32() < seconds {
        match rx.pop() {
            Ok(s) => raw.push(s),
            Err(_) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    drop(stream);

    // resample to 48k if needed
    let mut at48: Vec<f32> = Vec::with_capacity(raw.len());
    if rate == TARGET_RATE {
        at48 = raw.clone();
    } else if let Some((mut r, chunk_in, _co)) = make_resampler(rate, TARGET_RATE, rate / 100) {
        let mut inb = vec![vec![0.0f32; chunk_in]];
        let mut outb = vec![vec![0.0f32; r.output_frames_max()]];
        let mut src = &raw[..];
        while src.len() >= chunk_in {
            inb[0].copy_from_slice(&src[..chunk_in]);
            if let Ok((_u, w)) = r.process_into_buffer(&inb, &mut outb, None) {
                at48.extend_from_slice(&outb[0][..w]);
            }
            src = &src[chunk_in..];
        }
    } else {
        at48 = raw.clone(); // fall back: same-rate assumption
    }

    write_wav_i16(&stem.with_extension("raw.wav"), &at48, TARGET_RATE as u32)
        .map_err(|e| e.to_string())?;

    // run the full pipeline offline over the captured audio
    let mut chain = StudioChain::new(TARGET_RATE as f32, preset("studio"));
    let mut agc = Agc::new(chain.params().agc_target_db, enhanced);
    let mut denoiser = Denoiser::new().ok();
    let mut out: Vec<f32> = Vec::with_capacity(at48.len());
    let mut buf = vec![0.0f32; 480];
    let mut i = 0;
    while i + 480 <= at48.len() {
        buf.copy_from_slice(&at48[i..i + 480]);
        if enhanced {
            agc.process(&mut buf);
            if let Some(d) = denoiser.as_mut() {
                let _ = d.process(&mut buf);
            }
            chain.process(&mut buf);
        }
        out.extend_from_slice(&buf);
        i += 480;
    }
    write_wav_i16(
        &stem.with_extension("enhanced.wav"),
        &out,
        TARGET_RATE as u32,
    )
    .map_err(|e| e.to_string())?;

    Ok(format!(
        "device=\"{name}\" rate={rate} samples={} raw_rms_db={:.1} enhanced_rms_db={:.1}",
        at48.len(),
        rms_db(&at48),
        rms_db(&out)
    ))
}
