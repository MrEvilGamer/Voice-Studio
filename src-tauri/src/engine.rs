//! Real-time audio engine: capture → AGC → RNNoise → studio DSP → sinks.
//!
//! Threading model:
//! - cpal callbacks (RT): lock-free rtrb ring pushes/pops only.
//! - Processing thread: pulls input frames, runs AI + DSP, pushes to sink
//!   rings, publishes snapshots.
//! - Reconfigure thread: sole owner of cpal `Stream` lifetimes.
//! - Watcher thread: device polling, error/auto-recovery.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, StreamConfig, SupportedStreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{FftFixedInOut, Resampler};
use serde::Serialize;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ai::Denoiser;
use crate::dsp::{Agc, StudioChain, StudioParams};

pub const TARGET_RATE: usize = 48000;
pub const FRAME: usize = 480; // 10 ms @ 48 kHz

const INPUT_RING: usize = 19200; // 400 ms
const SINK_RING: usize = 9600; // 200 ms
const SINK_PREFILL: usize = 960; // 20 ms silent lead-in
const SINK_LOW: usize = 240; // 5 ms — below this, insert samples against clock drift

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineState {
    Stopped = 0,
    Starting = 1,
    Running = 2,
    Restarting = 3,
    Error = 4,
}

fn state_from_u8(v: u8) -> EngineState {
    match v {
        1 => EngineState::Starting,
        2 => EngineState::Running,
        3 => EngineState::Restarting,
        4 => EngineState::Error,
        _ => EngineState::Stopped,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub state: EngineState,
    pub input_rms_db: f32,
    pub input_peak_db: f32,
    pub output_rms_db: f32,
    pub output_peak_db: f32,
    pub clip: bool,
    pub noise_removed_db: f32,
    pub proc_ms: f32,
    pub pipeline_ms: f32,
    pub ai_active: bool,
    pub dsp_active: bool,
    pub agc_db: f32,
    pub vad: f32,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DevInfo {
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceLists {
    pub inputs: Vec<DevInfo>,
    pub outputs: Vec<DevInfo>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct DeviceSelection {
    pub input: Option<String>,
    pub monitor: Option<String>,
    pub virtual_out: Option<String>,
    pub buffer_frames: u32,
}

impl DeviceSelection {
    pub fn with_buffers(mut self, buffer_frames: u32) -> Self {
        self.buffer_frames = buffer_frames;
        self
    }
}

/// Real-time-safe control surface shared with the UI.
pub struct Controls {
    pub mode_enhanced: AtomicBool,
    pub blend_permille: AtomicU32, // 0..1000
    pub studio_on: AtomicBool,
    pub monitor_on: AtomicBool,
    pub monitor_vol_permille: AtomicU32, // 0..1000
    pub auto_gain: AtomicBool,
}

impl Controls {
    pub fn new() -> Self {
        Self {
            mode_enhanced: AtomicBool::new(true),
            blend_permille: AtomicU32::new(750),
            studio_on: AtomicBool::new(true),
            monitor_on: AtomicBool::new(false),
            monitor_vol_permille: AtomicU32::new(800),
            auto_gain: AtomicBool::new(true),
        }
    }
}

#[derive(Clone, Debug)]
pub enum DevEvent {
    ListChanged,
}

enum Cmd {
    Apply(DeviceSelection),
    Shutdown,
}

// ---------------------------------------------------------------------------
// Shared engine state
// ---------------------------------------------------------------------------

struct SharedInput {
    cons: Mutex<Consumer<f32>>,
    rate: Mutex<usize>,
    drain: AtomicBool,
}

struct SinkSlot {
    inner: Mutex<Option<SinkCore>>,
}

struct SinkRes {
    r: FftFixedInOut<f32>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
    chunk_in: usize,
}

struct SinkCore {
    tx: Producer<f32>,
    rate: usize,
    res: Option<SinkRes>,
    pending: Vec<f32>,
    vol: f32,
    vol_target: f32,
    drops: u64,
}

struct Shared {
    controls: Arc<Controls>,
    state: AtomicU8,
    message: Mutex<Option<String>>,
    latest: Arc<Mutex<Option<Snapshot>>>,
    input: Arc<SharedInput>,
    monitor: Arc<SinkSlot>,
    virt: Arc<SinkSlot>,
    shutdown: Arc<AtomicBool>,
    err_input: Arc<AtomicBool>,
}

impl Shared {
    fn set_state(&self, s: EngineState) {
        self.state.store(s as u8, Ordering::SeqCst);
    }
    fn set_msg(&self, m: Option<String>) {
        *self.message.lock().unwrap() = m;
    }
    fn append_msg(&self, m: &str) {
        let mut guard = self.message.lock().unwrap();
        match guard.as_mut() {
            Some(existing) => existing.push_str(&format!(" · {m}")),
            None => *guard = Some(m.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Resampler helpers (rubato FftFixedInOut, mono)
// ---------------------------------------------------------------------------

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Public mixdown helpers for the testbed (mono downmix into a ring).
pub fn mixdown_push_pub(data: &[f32], channels: usize, tx: &mut Producer<f32>) {
    mixdown_push(data, channels, tx, |s| *s);
}
pub fn mixdown_push_conv<T: Copy>(
    data: &[T],
    channels: usize,
    tx: &mut Producer<f32>,
    conv: impl Fn(&T) -> f32,
) {
    mixdown_push(data, channels, tx, conv);
}

/// Builds a mono FFT resampler whose chunks are whole multiples of the
/// reduced input ratio so in/out chunk sizes are exact integers.
pub(crate) fn make_resampler(
    from: usize,
    to: usize,
    approx_chunk: usize,
) -> Option<(FftFixedInOut<f32>, usize, usize)> {
    if from == to || from == 0 || to == 0 {
        return None;
    }
    let g = gcd(from, to);
    let n1 = from / g;
    let k = ((approx_chunk + n1 - 1) / n1).max(1);
    let chunk_in = n1 * k;
    let chunk_out = k * (to / g);
    match FftFixedInOut::new(from, to, chunk_in, 1) {
        Ok(r) => Some((r, chunk_in, chunk_out)),
        Err(_) => None,
    }
}

struct InputRes {
    r: FftFixedInOut<f32>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
    chunk_in: usize,
    carry: Vec<f32>,
    carry_pos: usize,
}

// ---------------------------------------------------------------------------
// Engine handle
// ---------------------------------------------------------------------------

pub struct Engine {
    pub controls: Arc<Controls>,
    shared: Arc<Shared>,
    sel: Arc<Mutex<DeviceSelection>>,
    cmd_tx: Sender<Cmd>,
    dsp_tx: Sender<StudioParams>,
    pub equalizers: Arc<Mutex<(crate::settings::EqConfig, crate::settings::EqConfig)>>,
    dev_rx: Mutex<Receiver<DevEvent>>,
}

impl Engine {
    pub fn new(initial: DeviceSelection, controls: Arc<Controls>) -> Arc<Self> {
        let (_iptx, icrx) = RingBuffer::<f32>::new(INPUT_RING);
        let shared = Arc::new(Shared {
            controls: controls.clone(),
            state: AtomicU8::new(EngineState::Starting as u8),
            message: Mutex::new(None),
            latest: Arc::new(Mutex::new(None)),
            input: Arc::new(SharedInput {
                cons: Mutex::new(icrx),
                rate: Mutex::new(TARGET_RATE),
                drain: AtomicBool::new(false),
            }),
            monitor: Arc::new(SinkSlot {
                inner: Mutex::new(None),
            }),
            virt: Arc::new(SinkSlot {
                inner: Mutex::new(None),
            }),
            shutdown: Arc::new(AtomicBool::new(false)),
            err_input: Arc::new(AtomicBool::new(false)),
        });

        let sel = Arc::new(Mutex::new(initial));
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let equalizers = Arc::new(Mutex::new((
            crate::settings::EqConfig::default(),
            crate::settings::EqConfig::default(),
        )));
        let process_eq = equalizers.clone();
        let (dsp_tx, dsp_rx) = channel::<StudioParams>();
        let (dev_tx, dev_rx) = channel::<DevEvent>();

        {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("voice-processing".into())
                .spawn(move || process_loop(shared, dsp_rx, process_eq))
                .expect("spawn processing thread");
        }
        {
            let shared = shared.clone();
            let sel = sel.clone();
            std::thread::Builder::new()
                .name("voice-reconfigure".into())
                .spawn(move || reconfigure_loop(shared, sel, cmd_rx))
                .expect("spawn reconfigure thread");
        }
        {
            let shared = shared.clone();
            let sel = sel.clone();
            let watcher_tx = cmd_tx.clone();
            std::thread::Builder::new()
                .name("voice-watcher".into())
                .spawn(move || watcher_loop(shared, sel, watcher_tx, dev_tx))
                .expect("spawn watcher thread");
        }

        let _ = cmd_tx.send(Cmd::Apply(sel.lock().unwrap().clone()));

        Arc::new(Self {
            controls,
            shared,
            sel,
            cmd_tx,
            dsp_tx,
            equalizers,
            dev_rx: Mutex::new(dev_rx),
        })
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        self.shared.latest.lock().unwrap().clone()
    }

    pub fn apply_devices(&self, sel: DeviceSelection) {
        let _ = self.cmd_tx.send(Cmd::Apply(sel));
    }

    pub fn set_preset_params(&self, p: StudioParams) {
        let _ = self.dsp_tx.send(p);
    }

    pub fn current_selection(&self) -> DeviceSelection {
        self.sel.lock().unwrap().clone()
    }

    pub fn poll_event(&self) -> Option<DevEvent> {
        self.dev_rx.lock().unwrap().try_recv().ok()
    }

    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(Cmd::Shutdown);
    }

    pub fn list_devices() -> DeviceLists {
        let host = cpal::default_host();
        let def_in = host.default_input_device().and_then(|d| d.name().ok());
        let def_out = host.default_output_device().and_then(|d| d.name().ok());
        let inputs = host
            .input_devices()
            .map(|it| {
                it.filter_map(|d| d.name().ok())
                    .map(|name| DevInfo {
                        is_default: Some(&name) == def_in.as_ref(),
                        name,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let outputs = host
            .output_devices()
            .map(|it| {
                it.filter_map(|d| d.name().ok())
                    .map(|name| DevInfo {
                        is_default: Some(&name) == def_out.as_ref(),
                        name,
                    })
                    .collect()
            })
            .unwrap_or_default();
        DeviceLists { inputs, outputs }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Stream building (called from reconfigure thread only)
// ---------------------------------------------------------------------------

fn find_input(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, String> {
    match name {
        Some(n) => host
            .input_devices()
            .map_err(|e| format!("cannot enumerate inputs: {e}"))?
            .find(|d| d.name().ok().as_deref() == Some(n))
            .ok_or_else(|| format!("Microphone \"{n}\" not found")),
        None => host
            .default_input_device()
            .ok_or_else(|| "No microphone found".to_string()),
    }
}

fn find_output(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, String> {
    match name {
        Some(n) => host
            .output_devices()
            .map_err(|e| format!("cannot enumerate outputs: {e}"))?
            .find(|d| d.name().ok().as_deref() == Some(n))
            .ok_or_else(|| format!("Output device \"{n}\" not found")),
        None => host
            .default_output_device()
            .ok_or_else(|| "No output device found".to_string()),
    }
}

#[inline(always)]
fn mixdown_push<T: Copy>(
    data: &[T],
    channels: usize,
    tx: &mut Producer<f32>,
    conv: impl Fn(&T) -> f32,
) {
    if channels <= 1 {
        for s in data {
            let _ = tx.push(conv(s));
        }
    } else {
        for fr in data.chunks(channels) {
            let mut acc = 0.0f32;
            let mut n = 0.0f32;
            for s in fr {
                acc += conv(s);
                n += 1.0;
            }
            let _ = tx.push(acc / n);
        }
    }
}

fn build_input_stream(
    dev: &cpal::Device,
    cfg: &SupportedStreamConfig,
    err_flag: Arc<AtomicBool>,
) -> Result<(cpal::Stream, Consumer<f32>, usize, usize), String> {
    let sc: StreamConfig = cfg.config();
    let channels = sc.channels as usize;
    let result = match cfg.sample_format() {
        SampleFormat::F32 => {
            let (mut tx, mut rx) = RingBuffer::<f32>::new(INPUT_RING);
            let err = err_flag.clone();
            dev.build_input_stream(
                &sc,
                move |d: &[f32], _: &cpal::InputCallbackInfo| {
                    mixdown_push(d, channels, &mut tx, |s| *s);
                },
                move |_: cpal::StreamError| err.store(true, Ordering::Relaxed),
                None,
            )
            .map(|s| (s, rx))
        }
        SampleFormat::I16 => {
            let (mut tx, mut rx) = RingBuffer::<f32>::new(INPUT_RING);
            let err = err_flag.clone();
            dev.build_input_stream(
                &sc,
                move |d: &[i16], _: &cpal::InputCallbackInfo| {
                    mixdown_push(d, channels, &mut tx, |s| *s as f32 / 32768.0);
                },
                move |_: cpal::StreamError| err.store(true, Ordering::Relaxed),
                None,
            )
            .map(|s| (s, rx))
        }
        SampleFormat::U16 => {
            let (mut tx, mut rx) = RingBuffer::<f32>::new(INPUT_RING);
            let err = err_flag.clone();
            dev.build_input_stream(
                &sc,
                move |d: &[u16], _: &cpal::InputCallbackInfo| {
                    mixdown_push(d, channels, &mut tx, |s| (*s as f32 - 32768.0) / 32768.0);
                },
                move |_: cpal::StreamError| err.store(true, Ordering::Relaxed),
                None,
            )
            .map(|s| (s, rx))
        }
        _ => return Err("unsupported input sample format".to_string()),
    };
    match result {
        Ok((stream, rx)) => Ok((stream, rx, sc.sample_rate.0 as usize, channels)),
        Err(e) => Err(format!("input stream: {e}")),
    }
}

fn build_output_try(
    dev: &cpal::Device,
    cfg: &SupportedStreamConfig,
    sc: &StreamConfig,
    channels: usize,
) -> Result<(cpal::Stream, Producer<f32>, usize, usize), String> {
    let result = match cfg.sample_format() {
        SampleFormat::F32 => {
            let (mut tx, mut rx) = RingBuffer::<f32>::new(SINK_RING);
            let mut fade = OutputFade {
                gain: 0.0,
                last: 0.0,
            };
            dev.build_output_stream(
                sc,
                move |d: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    fill_output(d, channels, &mut rx, &mut fade, |s| s);
                },
                |_: cpal::StreamError| {},
                None,
            )
            .map(|s| (s, tx))
        }
        SampleFormat::I16 => {
            let (mut tx, mut rx) = RingBuffer::<f32>::new(SINK_RING);
            let mut fade = OutputFade {
                gain: 0.0,
                last: 0.0,
            };
            dev.build_output_stream(
                sc,
                move |d: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    fill_output(d, channels, &mut rx, &mut fade, |s| (s * 32767.0) as i16);
                },
                |_: cpal::StreamError| {},
                None,
            )
            .map(|s| (s, tx))
        }
        SampleFormat::U16 => {
            let (mut tx, mut rx) = RingBuffer::<f32>::new(SINK_RING);
            let mut fade = OutputFade {
                gain: 0.0,
                last: 0.0,
            };
            dev.build_output_stream(
                sc,
                move |d: &mut [u16], _: &cpal::OutputCallbackInfo| {
                    fill_output(d, channels, &mut rx, &mut fade, |s| {
                        ((s * 32767.0) + 32768.0) as u16
                    });
                },
                |_: cpal::StreamError| {},
                None,
            )
            .map(|s| (s, tx))
        }
        _ => return Err("unsupported output sample format".to_string()),
    };
    match result {
        Ok((stream, tx)) => Ok((stream, tx, sc.sample_rate.0 as usize, channels)),
        Err(e) => Err(format!("output stream: {e}")),
    }
}

fn build_output_stream(
    dev: &cpal::Device,
    cfg: &SupportedStreamConfig,
    buffer_frames: u32,
) -> Result<(cpal::Stream, Producer<f32>, usize, usize), String> {
    let channels = cfg.channels() as usize;
    let mut sc = cfg.config();
    sc.buffer_size = BufferSize::Fixed(buffer_frames);
    match build_output_try(dev, cfg, &sc, channels) {
        Ok(v) => Ok(v),
        Err(_) => {
            // Some devices/drivers reject fixed buffer sizes — use default.
            let mut sc2 = cfg.config();
            sc2.buffer_size = BufferSize::Default;
            build_output_try(dev, cfg, &sc2, channels)
        }
    }
}

/// Writes mono samples from the ring into an interleaved output buffer,
/// duplicating the mono sample into every channel of each frame. A short
/// fade hides ring underruns.
struct OutputFade {
    gain: f32,
    last: f32,
}

fn fill_output<T: Copy>(
    data: &mut [T],
    channels: usize,
    rx: &mut Consumer<f32>,
    fade: &mut OutputFade,
    conv: impl Fn(f32) -> T,
) {
    if channels <= 1 {
        for slot in data.iter_mut() {
            let s = match rx.pop() {
                Ok(s) => {
                    fade.gain += 0.02 * (1.0 - fade.gain);
                    fade.last = s * fade.gain;
                    fade.last
                }
                Err(_) => {
                    fade.gain = 0.0;
                    fade.last *= 0.98;
                    fade.last
                }
            };
            *slot = conv(s);
        }
    } else {
        for fr in data.chunks_mut(channels) {
            let s = match rx.pop() {
                Ok(s) => {
                    fade.gain += 0.02 * (1.0 - fade.gain);
                    fade.last = s * fade.gain;
                    fade.last
                }
                Err(_) => {
                    fade.gain = 0.0;
                    fade.last *= 0.98;
                    fade.last
                }
            };
            for slot in fr.iter_mut() {
                *slot = conv(s);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reconfigure thread
// ---------------------------------------------------------------------------

struct Rig {
    input: Option<cpal::Stream>,
    monitor: Option<cpal::Stream>,
    virt: Option<cpal::Stream>,
}

fn reconfigure_loop(shared: Arc<Shared>, sel: Arc<Mutex<DeviceSelection>>, cmd_rx: Receiver<Cmd>) {
    let mut rig = Rig {
        input: None,
        monitor: None,
        virt: None,
    };
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Cmd::Shutdown => break,
            Cmd::Apply(s) => {
                if shared.shutdown.load(Ordering::SeqCst) {
                    break;
                }
                *sel.lock().unwrap() = s.clone();
                apply_selection(&shared, &mut rig, &s);
            }
        }
    }
    rig.input = None;
    rig.monitor = None;
    rig.virt = None;
}

fn apply_selection(shared: &Shared, rig: &mut Rig, sel: &DeviceSelection) {
    log::info!(
        "apply selection: input={:?} monitor={:?} virtual={:?} buf={}",
        sel.input,
        sel.monitor,
        sel.virtual_out,
        sel.buffer_frames
    );
    shared.set_state(EngineState::Restarting);

    // teardown everything first
    rig.input = None;
    rig.monitor = None;
    rig.virt = None;
    *shared.monitor.inner.lock().unwrap() = None;
    *shared.virt.inner.lock().unwrap() = None;
    shared.set_msg(None);

    let host = cpal::default_host();
    let mut input_ok = false;

    // ---- input ----
    match find_input(&host, sel.input.as_deref()).and_then(|dev| {
        dev.default_input_config()
            .map_err(|e| format!("input config: {e}"))
            .map(|c| (dev, c))
    }) {
        Ok((dev, cfg)) => match build_input_stream(&dev, &cfg, shared.err_input.clone()) {
            Ok((stream, cons, rate, _ch)) => {
                log::info!(
                    "input: \"{}\" rate={rate} fmt={:?}",
                    dev.name().unwrap_or_default(),
                    cfg.sample_format()
                );
                {
                    let mut g = shared.input.cons.lock().unwrap();
                    *g = cons;
                }
                *shared.input.rate.lock().unwrap() = rate;
                shared.input.drain.store(true, Ordering::SeqCst);
                match stream.play() {
                    Ok(()) => {
                        rig.input = Some(stream);
                        input_ok = true;
                    }
                    Err(e) => {
                        log::error!("input start failed: {e}");
                        shared.set_msg(Some(format!("input start: {e}")));
                    }
                }
            }
            Err(e) => {
                log::error!("input stream build failed: {e}");
                shared.set_msg(Some(e));
            }
        },
        Err(e) => {
            log::error!("input: {e}");
            shared.set_msg(Some(e));
        }
    }

    // ---- monitor sink ----
    match open_sink(&host, sel.monitor.as_deref(), sel.buffer_frames) {
        Ok((stream, core, name, rate)) => {
            log::info!("monitor sink: \"{name}\" rate={rate}");
            *shared.monitor.inner.lock().unwrap() = Some(core);
            rig.monitor = Some(stream);
        }
        Err(e) => log::warn!("monitor sink unavailable: {e}"),
    }

    // ---- virtual sink ----
    if let Some(vname) = sel.virtual_out.clone() {
        match open_sink(&host, Some(&vname), sel.buffer_frames) {
            Ok((stream, core, name, rate)) => {
                log::info!("virtual sink: \"{name}\" rate={rate}");
                *shared.virt.inner.lock().unwrap() = Some(core);
                rig.virt = Some(stream);
            }
            Err(e) => {
                log::warn!("virtual sink unavailable: {e}");
                shared.append_msg(&format!("Virtual output unavailable: {e}"));
            }
        }
    }

    if input_ok {
        shared.set_state(EngineState::Running);
    } else {
        shared.set_state(EngineState::Error);
    }
}

fn open_sink(
    host: &cpal::Host,
    name: Option<&str>,
    buffer_frames: u32,
) -> Result<(cpal::Stream, SinkCore, String, usize), String> {
    let dev = find_output(host, name)?;
    let dev_name = dev.name().unwrap_or_default();
    let cfg = dev
        .default_output_config()
        .map_err(|e| format!("output config: {e}"))?;
    let (stream, tx, rate, _ch) = build_output_stream(&dev, &cfg, buffer_frames)?;
    stream.play().map_err(|e| format!("output start: {e}"))?;

    let mut core = SinkCore {
        tx,
        rate,
        res: None,
        pending: Vec::new(),
        vol: 1.0,
        vol_target: 1.0,
        drops: 0,
    };
    if let Some((r, chunk_in, _co)) = make_resampler(TARGET_RATE, rate, FRAME) {
        let max_out = r.output_frames_max();
        log::info!("sink resampler: {TARGET_RATE} -> {rate} (chunks {chunk_in} -> {max_out})");
        core.res = Some(SinkRes {
            r,
            in_buf: vec![vec![0.0; chunk_in]],
            out_buf: vec![vec![0.0; max_out]],
            chunk_in,
        });
        core.pending = Vec::with_capacity(chunk_in * 2);
    }
    for _ in 0..SINK_PREFILL {
        let _ = core.tx.push(0.0);
    }
    Ok((stream, core, dev_name, rate))
}

// ---------------------------------------------------------------------------
// Watcher thread
// ---------------------------------------------------------------------------

fn watcher_loop(
    shared: Arc<Shared>,
    sel: Arc<Mutex<DeviceSelection>>,
    cmd_tx: Sender<Cmd>,
    dev_tx: Sender<DevEvent>,
) {
    let mut prev_inputs: Vec<String> = Vec::new();
    let mut prev_outputs: Vec<String> = Vec::new();
    let mut first = true;
    loop {
        std::thread::sleep(Duration::from_millis(2000));
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        let lists = Engine::list_devices();
        let in_names: Vec<String> = lists.inputs.iter().map(|d| d.name.clone()).collect();
        let out_names: Vec<String> = lists.outputs.iter().map(|d| d.name.clone()).collect();

        if first || in_names != prev_inputs || out_names != prev_outputs {
            let _ = dev_tx.send(DevEvent::ListChanged);
            prev_inputs = in_names.clone();
            prev_outputs = out_names.clone();
            first = false;
        }

        let err = shared.err_input.swap(false, Ordering::Relaxed);
        let state = shared.state.load(Ordering::SeqCst);
        let sel_now = sel.lock().unwrap().clone();
        let input_gone = match &sel_now.input {
            Some(name) => !in_names.contains(name),
            None => false,
        };
        let monitor_gone = match &sel_now.monitor {
            Some(name) => !out_names.contains(name),
            None => false,
        };
        if err || state == EngineState::Error as u8 || input_gone || monitor_gone {
            log::info!(
                "watcher: re-apply (err={err} state={state} input_gone={input_gone} monitor_gone={monitor_gone})"
            );
            let _ = cmd_tx.send(Cmd::Apply(sel_now));
        }
    }
}

// ---------------------------------------------------------------------------
// Processing thread
// ---------------------------------------------------------------------------

fn process_loop(
    shared: Arc<Shared>,
    dsp_rx: Receiver<StudioParams>,
    equalizers: Arc<Mutex<(crate::settings::EqConfig, crate::settings::EqConfig)>>,
) {
    let controls = shared.controls.clone();

    let mut denoiser = match Denoiser::new() {
        Ok(d) => {
            log::info!("RNNoise ready, frame_size={}", d.frame_size());
            Some(d)
        }
        Err(e) => {
            log::error!("RNNoise init failed: {e}");
            None
        }
    };
    let n = denoiser
        .as_ref()
        .map(|d| d.frame_size())
        .unwrap_or(FRAME)
        .max(FRAME);
    let mut ai_fallback = denoiser.is_none();
    let mut dsp_fallback = false;
    if ai_fallback {
        shared.set_msg(Some(
            "AI model failed to load — running Studio DSP only".into(),
        ));
    }

    let studio_params = crate::dsp::preset("studio");
    let mut chain = StudioChain::new(TARGET_RATE as f32, studio_params);
    let mut agc = Agc::new(studio_params.agc_target_db, true);

    let mut mic_eq = crate::dsp::GraphicEq::new(TARGET_RATE as f32);
    let mut speaker_eq = crate::dsp::GraphicEq::new(TARGET_RATE as f32);
    let mut monitor_frame = vec![0.0f32; n];
    let mut frame = vec![0.0f32; n];
    let mut dry = vec![0.0f32; n];
    let mut ai_out = vec![0.0f32; n];
    let mut dsp_out = vec![0.0f32; n];

    let mut g_mode = if controls.mode_enhanced.load(Ordering::Relaxed) {
        1.0
    } else {
        0.0
    };
    let mut g_studio = if controls.studio_on.load(Ordering::Relaxed) {
        1.0
    } else {
        0.0
    };

    let mut cur_in_rate = TARGET_RATE;
    let mut in_res: Option<InputRes> = None;

    let mut in_rms_db = -100.0f32;
    let mut in_peak = 0.0f32;
    let mut out_rms_db = -100.0f32;
    let mut out_peak = 0.0f32;
    let mut noise_db = 0.0f32;
    let mut proc_ms = 0.0f32;
    let mut pipeline_ms = 0.0f32;
    let mut vad = 0.0f32;
    let mut clip_until = Instant::now() - Duration::from_secs(10);
    let mut last_snap = Instant::now() - Duration::from_secs(1);
    let mut in_occ = 0usize;
    let mut mon_occ = 0usize;

    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }
        if let Ok(p) = dsp_rx.try_recv() {
            agc.set_target(p.agc_target_db);
            chain.set_params(p);
            log::info!("studio params updated");
        }

        // input resampler follows device rate changes
        let rate_now = *shared.input.rate.lock().unwrap();
        if rate_now != cur_in_rate {
            cur_in_rate = rate_now;
            in_res =
                make_resampler(rate_now, TARGET_RATE, rate_now / 100).map(|(r, chunk_in, _co)| {
                    let max_out = r.output_frames_max();
                    InputRes {
                        r,
                        in_buf: vec![vec![0.0; chunk_in]],
                        out_buf: vec![vec![0.0; max_out]],
                        chunk_in,
                        carry: Vec::with_capacity(chunk_in * 2),
                        carry_pos: 0,
                    }
                });
            if rate_now != TARGET_RATE {
                log::info!("input resampler: {rate_now} -> {TARGET_RATE}");
            }
        }

        if shared.input.drain.swap(false, Ordering::SeqCst) {
            let mut cons = shared.input.cons.lock().unwrap();
            while cons.pop().is_ok() {}
        }

        let (got, occ) = pull_frame(&shared.input, &mut in_res, &mut frame);
        in_occ = occ;
        if !got {
            std::thread::sleep(Duration::from_millis(1));
            if last_snap.elapsed() > Duration::from_millis(150) {
                publish(
                    shared.clone(),
                    shared.state.load(Ordering::SeqCst),
                    shared.message.lock().unwrap().clone(),
                    in_rms_db,
                    dbf(in_peak),
                    out_rms_db,
                    dbf(out_peak),
                    Instant::now() < clip_until,
                    noise_db,
                    proc_ms,
                    pipeline_ms,
                    !ai_fallback && denoiser.is_some(),
                    !dsp_fallback,
                    agc.gain_db(),
                    vad,
                );
                last_snap = Instant::now();
            }
            continue;
        }

        let t0 = Instant::now();

        dry.copy_from_slice(&frame);
        in_peak = dry.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let in_rms = rms(&dry);
        in_rms_db = dbf(in_rms);
        if in_peak >= 0.985 {
            clip_until = Instant::now() + Duration::from_secs(2);
        }

        // AGC (enhanced-path input conditioning)
        agc.enabled = controls.auto_gain.load(Ordering::Relaxed);
        agc.process(&mut frame);

        // AI noise suppression
        let ai_active = !ai_fallback && denoiser.is_some();
        if ai_active {
            let d = denoiser.as_mut().unwrap();
            match catch_unwind(AssertUnwindSafe(|| d.process(&mut frame))) {
                Ok(v) => vad = v,
                Err(_) => {
                    ai_fallback = true;
                    log::error!("RNNoise panicked — falling back to DSP-only");
                    shared.set_msg(Some(
                        "AI noise removal failed — running Studio DSP only".into(),
                    ));
                }
            }
        }

        // wet/dry blend of the AI stage
        let blend = controls.blend_permille.load(Ordering::Relaxed) as f32 / 1000.0;
        for i in 0..n {
            ai_out[i] = dry[i] + (frame[i] - dry[i]) * blend;
        }

        // noise-removed estimate: attenuation actually achieved post-blend
        let ai_rms = rms(&ai_out);
        if in_rms_db > -75.0 && ai_rms > 1e-9 {
            let inst = (20.0 * (in_rms / ai_rms).log10()).clamp(0.0, 40.0);
            let coef = if inst > noise_db { 0.06 } else { 0.02 };
            noise_db += coef * (inst - noise_db);
        } else {
            noise_db *= 0.995;
        }

        // studio chain
        dsp_out.copy_from_slice(&ai_out);
        if !dsp_fallback {
            let res = catch_unwind(AssertUnwindSafe(|| chain.process(&mut dsp_out)));
            if res.is_err() {
                dsp_fallback = true;
                log::error!("studio DSP panicked — falling back to passthrough");
                shared.set_msg(Some("Studio DSP failed — running passthrough".into()));
                dsp_out.copy_from_slice(&ai_out);
            }
        }

        // assemble with short crossfades (click-free RAW/ENHANCED + studio toggling)
        let gm_t = if controls.mode_enhanced.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        let gs_t = if controls.studio_on.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        let d_mode = (gm_t - g_mode) / n as f32;
        let d_studio = (gs_t - g_studio) / n as f32;
        let mut gm = g_mode;
        let mut gs = g_studio;
        for i in 0..n {
            let enhanced = ai_out[i] + (dsp_out[i] - ai_out[i]) * gs;
            let y = dry[i] + (enhanced - dry[i]) * gm;
            frame[i] = y;
            gm += d_mode;
            gs += d_studio;
        }
        g_mode = gm_t;
        g_studio = gs_t;

        let eq = equalizers.lock().unwrap().clone();
        mic_eq.process(&mut frame, &eq.0);
        monitor_frame.copy_from_slice(&frame);
        speaker_eq.process(&mut monitor_frame, &eq.1);
        out_peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        out_rms_db = dbf(rms(&frame));

        // push sinks
        let mon_gain = if controls.monitor_on.load(Ordering::Relaxed) {
            controls.monitor_vol_permille.load(Ordering::Relaxed) as f32 / 1000.0
        } else {
            0.0
        };
        {
            let mut slot = shared.monitor.inner.lock().unwrap();
            if let Some(core) = slot.as_mut() {
                push_sink(core, &monitor_frame, mon_gain);
                mon_occ = SINK_RING - core.tx.slots();
            } else {
                mon_occ = 0;
            }
        }
        {
            let mut slot = shared.virt.inner.lock().unwrap();
            if let Some(core) = slot.as_mut() {
                push_sink(core, &frame, 1.0);
            }
        }

        // our own virtual microphone driver (inert until driver/ is installed)
        crate::vsmic::push_frame(&frame);

        proc_ms = proc_ms * 0.9 + t0.elapsed().as_secs_f32() * 1000.0 * 0.1;
        pipeline_ms = (in_occ + n + mon_occ) as f32 / TARGET_RATE as f32 * 1000.0;

        if last_snap.elapsed() > Duration::from_millis(100) {
            publish(
                shared.clone(),
                shared.state.load(Ordering::SeqCst),
                shared.message.lock().unwrap().clone(),
                in_rms_db,
                dbf(in_peak),
                out_rms_db,
                dbf(out_peak),
                Instant::now() < clip_until,
                noise_db,
                proc_ms,
                pipeline_ms,
                !ai_fallback && denoiser.is_some(),
                !dsp_fallback,
                agc.gain_db(),
                vad,
            );
            last_snap = Instant::now();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn publish(
    shared: Arc<Shared>,
    state: u8,
    message: Option<String>,
    in_rms_db: f32,
    in_peak_db: f32,
    out_rms_db: f32,
    out_peak_db: f32,
    clip: bool,
    noise_db: f32,
    proc_ms: f32,
    pipeline_ms: f32,
    ai_active: bool,
    dsp_active: bool,
    agc_db: f32,
    vad: f32,
) {
    let snap = Snapshot {
        state: state_from_u8(state),
        input_rms_db: in_rms_db,
        input_peak_db: in_peak_db,
        output_rms_db: out_rms_db,
        output_peak_db: out_peak_db,
        clip,
        noise_removed_db: noise_db,
        proc_ms,
        pipeline_ms,
        ai_active,
        dsp_active,
        agc_db,
        vad,
        message,
    };
    *shared.latest.lock().unwrap() = Some(snap);
}

#[inline]
fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
}

#[inline]
fn dbf(v: f32) -> f32 {
    if v > 1e-9 {
        20.0 * v.log10()
    } else {
        -100.0
    }
}

fn pull_frame(input: &SharedInput, res: &mut Option<InputRes>, out: &mut [f32]) -> (bool, usize) {
    let mut filled = 0usize;
    let mut cons = input.cons.lock().unwrap();
    let occ_start = cons.slots();
    match res {
        None => {
            if cons.slots() < out.len() {
                return (false, occ_start);
            }
            while filled < out.len() {
                match cons.pop() {
                    Ok(s) => {
                        out[filled] = s;
                        filled += 1;
                    }
                    Err(_) => return (false, occ_start),
                }
            }
            (true, occ_start)
        }
        Some(r) => loop {
            while r.carry_pos < r.carry.len() && filled < out.len() {
                out[filled] = r.carry[r.carry_pos];
                r.carry_pos += 1;
                filled += 1;
            }
            if filled == out.len() {
                if r.carry_pos >= r.carry.len() {
                    r.carry.clear();
                    r.carry_pos = 0;
                } else if r.carry_pos > 0 {
                    r.carry.drain(..r.carry_pos);
                    r.carry_pos = 0;
                }
                return (true, occ_start);
            }
            if cons.slots() < r.chunk_in {
                if filled > 0 {
                    r.carry = out[..filled].to_vec();
                    r.carry_pos = 0;
                }
                return (false, occ_start);
            }
            r.carry.clear();
            r.carry_pos = 0;
            for i in 0..r.chunk_in {
                r.in_buf[0][i] = cons.pop().unwrap_or(0.0);
            }
            match r.r.process_into_buffer(&r.in_buf, &mut r.out_buf, None) {
                Ok((_used, written)) => {
                    r.carry.extend_from_slice(&r.out_buf[0][..written]);
                    r.carry_pos = 0;
                }
                Err(e) => {
                    log::error!("resample error: {e}");
                    return (false, occ_start);
                }
            }
        },
    }
}

fn push_sink(core: &mut SinkCore, frame: &[f32], vol_target: f32) {
    core.vol_target = vol_target;

    if core.res.is_none() {
        push_samples(core, frame);
        return;
    }
    core.pending.extend_from_slice(frame);
    loop {
        let Some(res) = core.res.as_mut() else { break };
        if core.pending.len() < res.chunk_in {
            break;
        }
        res.in_buf[0].copy_from_slice(&core.pending[..res.chunk_in]);
        let written = res
            .r
            .process_into_buffer(&res.in_buf, &mut res.out_buf, None)
            .map(|(_u, w)| w)
            .unwrap_or(0);
        let chunk: Vec<f32> = if written > 0 {
            res.out_buf[0][..written].to_vec()
        } else {
            Vec::new()
        };
        core.pending.drain(..res.chunk_in);
        if !chunk.is_empty() {
            push_samples(core, &chunk);
        }
    }
}

fn push_samples(core: &mut SinkCore, data: &[f32]) {
    if data.is_empty() {
        return;
    }
    // Spread clock correction across a block instead of inserting an abrupt tail.
    let occupancy = SINK_RING - core.tx.slots();
    let correction: isize = if occupancy < SINK_LOW {
        1
    } else if occupancy > SINK_PREFILL * 3 {
        -1
    } else {
        0
    };
    let count = (data.len() as isize + correction).max(1) as usize;
    let dv = (core.vol_target - core.vol) / count as f32;
    let mut v = core.vol;
    for i in 0..count {
        let pos = i as f32 * (data.len() - 1) as f32 / (count - 1).max(1) as f32;
        let lo = pos.floor() as usize;
        let s = data[lo] + (data[(lo + 1).min(data.len() - 1)] - data[lo]) * (pos - lo as f32);
        v += dv;
        if core.tx.push(s * v).is_err() {
            core.drops += 1;
            break;
        }
    }
    core.vol = v;
}
#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn incomplete_capture_is_preserved() {
        let (mut tx, rx) = RingBuffer::<f32>::new(INPUT_RING);
        let input = SharedInput {
            cons: Mutex::new(rx),
            rate: Mutex::new(TARGET_RATE),
            drain: AtomicBool::new(false),
        };
        for i in 0..120 {
            tx.push(i as f32).unwrap();
        }
        let mut output = [0.0; FRAME];
        assert!(!pull_frame(&input, &mut None, &mut output).0);
        assert_eq!(input.cons.lock().unwrap().slots(), 120);
        for i in 120..FRAME {
            tx.push(i as f32).unwrap();
        }
        assert!(pull_frame(&input, &mut None, &mut output).0);
        for i in 0..FRAME {
            assert_eq!(output[i], i as f32);
        }
    }
    #[test]
    fn underrun_fades_last_sample() {
        let (mut tx, mut rx) = RingBuffer::<f32>::new(100);
        tx.push(0.5).unwrap();
        let mut fade = OutputFade {
            gain: 1.0,
            last: 0.0,
        };
        let mut output = [0.0; 4];
        fill_output(&mut output, 1, &mut rx, &mut fade, |s| s);
        assert_eq!(output[0], 0.5);
        assert!(output[1] > 0.45 && output[1] < 0.5);
        assert!(output[3] < output[2]);
    }
    #[test]
    fn resampled_partial_frame_is_preserved() {
        let (mut tx, rx) = RingBuffer::<f32>::new(INPUT_RING);
        let input = SharedInput {
            cons: Mutex::new(rx),
            rate: Mutex::new(44100),
            drain: AtomicBool::new(false),
        };
        let (r, chunk_in, _) = make_resampler(44100, TARGET_RATE, 441).unwrap();
        let max_out = r.output_frames_max();
        let mut res = Some(InputRes {
            r,
            in_buf: vec![vec![0.0; chunk_in]],
            out_buf: vec![vec![0.0; max_out]],
            chunk_in,
            carry: vec![0.123; 120],
            carry_pos: 0,
        });
        let mut out = [0.0; FRAME];
        assert!(!pull_frame(&input, &mut res, &mut out).0);
        assert_eq!(res.as_ref().unwrap().carry.len(), 120);
        for _ in 0..chunk_in {
            tx.push(0.0).unwrap();
        }
        assert!(pull_frame(&input, &mut res, &mut out).0);
        assert!(out[..120].iter().all(|v| (*v - 0.123).abs() < 0.00001));
    }
}
