# Voice Studio v0.1

Real-time AI microphone enhancement for Windows. Takes any microphone, removes
background noise with RNNoise (a real-time neural noise-suppression model,
running fully locally), applies a conservative studio chain (high-pass → EQ →
de-esser → compressor → limiter with slow auto input gain), lets you monitor
yourself live, and routes the processed audio to any Windows playback endpoint
(e.g. a virtual cable input) so Discord / FiveM / OBS can use it as a clean
microphone.

Built with Tauri 2 + React/TypeScript (UI) and Rust (audio engine, cpal/WASAPI,
RNNoise vendored and compiled from source, no network access needed at runtime).

## Run

- Dev: `npm install`, then `npm run tauri dev`
- Build: `npm run tauri build` → installer in `src-tauri/target/release/bundle/nsis/`,
  standalone exe at `src-tauri/target/release/voice-studio.exe`

## Using it with Discord / FiveM / OBS

1. Install a virtual audio device if you don't have one (e.g. VB-CABLE).
2. Voice Studio detects and routes the virtual device automatically.
3. In Discord / FiveM / OBS: select the matching *recording* endpoint
   (e.g. "CABLE Output") as your microphone.
4. **Listen To Myself** (monitoring) is off by default — turn it on to hear the
   processed signal in real time. Headphones recommended.

## Verification (automated)

- `cargo test` — DSP unit tests (filters, de-esser, compressor, limiter, AGC)
  and an RNNoise integration test.
- `cargo run --release --example pipeline_test` — offline A/B: real TTS speech
  mixed with fan/hum/keyboard noise; asserts ≥8 dB noise reduction with the
  voice band preserved, limiter ceiling held. Writes `pipeline_in.wav` /
  `pipeline_out.wav` to `%TEMP%\voice-studio-test`.
- `cargo run --release --example live_test` — enumerates devices and captures
  3 s of live microphone through the full pipeline into WAVs.

## Latency

The UI shows measured **Processing** time per 10 ms frame (RNNoise + studio
DSP) and the current **engine buffer** depth. Frame size is 10 ms; output
buffer is configurable (Low latency / Balanced / Safe).

## Notes

- All processing is local. No microphone audio ever leaves the machine; no
  account, no telemetry.
- If the AI fails at runtime the engine falls back to Studio-DSP-only; if the
  DSP fails, to passthrough. The UI shows a warning when that happens.
- Settings persist in `%APPDATA%\Voice Studio\settings.json`.
  Logging in the same folder (`voice-studio.log`) — metadata only, never audio.

## Desktop redesign and equalizers

The desktop shell follows Flare's dark slate / orange styling, with a sidebar and custom window controls. Right-click browser menus are suppressed.

Voice presets: Natural, Studio, Gaming, Broadcast, Podcast, Warm, Clarity, Meeting. Presets shape the high-pass filter, tonal EQ, de-esser and compressor. Hover for their intended use.

Equalizers provides independent ten-band microphone and monitoring-output EQ, bypass, preamp, Flat/Warm/Clarity/Vocal/Bass/Gaming/Podcast curves, and validated JSON import/export. The microphone EQ reaches connected apps; the speaker EQ affects Voice Studio monitoring only. It does not process other Windows application playback. Boosts automatically reserve headroom.

Audio regression tests cover incomplete capture frames, partial resampler frames, underrun fades, silence gain, EQ, preset stability, limiter bounds and RNNoise.

Commercial release still requires completing and signing the bundled virtual microphone driver and validating hardware latency and listening quality on multiple devices. The current virtual microphone fallback is a development dependency.

## Brand and Flare shell

The original fiery VS emblem is in `public/brand/voice-studio-emblem.png`; its generation prompt is saved in `public/brand/README.md`. `python scripts/build-icons.py` exports the UI logo and native PNG/ICO sizes. Icon packaging uses Pillow and preserves transparency.

The shell now follows Flare's actual structure: 32 px titlebar, 64 px icon rail expanding to 216 px on hover or keyboard focus, 52 px fixed top bar, independent centered page scrolling, open three-column dashboard panels, and matching slate/orange surfaces. SVG navigation icons replace text glyphs. Sora, Space Grotesk and IBM Plex Mono fonts are bundled locally with their OFL licenses; regenerate with `python scripts/bundle-fonts.py`.

## Eye tracking

Eye Tracking imports the local Eye Tracker engine into the same desktop shell. It includes a live camera preview, eye/head calibration, accuracy checks, gaze overlay, optional mouse control, head smoothing and local OpenTrack UDP output. Start/Stop controls camera access; the voice engine remains independent. F8 recenters, F9 pauses and F10 toggles mouse control while the tracker is running. Calibration uses a full-screen target sequence that Escape cancels.

Personal calibration and settings are stored in `%APPDATA%\Voice Studio\Eye Tracker`. The installer bundles the Python tracker runtime; Python is only needed to rebuild it. Run `powershell -File scripts/build-eye-tracker.ps1` before `npm run tauri build`. See `tracker/README.md` for details.
