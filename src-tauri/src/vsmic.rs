//! Transport between the audio engine and the Voice Studio virtual microphone
//! kernel driver (source lives in `driver/`).
//!
//! Protocol: the driver exposes a control device `\\.\VoiceStudioMic`.
//!   IOCTL_VSM_GET_VERSION  — handshake, returns protocol version
//!   IOCTL_VSM_PUSH_AUDIO   — payload: N × f32 samples @ 48 kHz mono
//! The engine calls [`push_frame`] once per processed 10 ms frame; a writer
//! thread owns the device handle. Everything is inert until the driver is
//! actually installed on the machine.

use rtrb::RingBuffer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const DEVICE_PATH: &str = "\\\\.\\VoiceStudioMic";

const IOCTL_VSM_GET_VERSION: u32 = ctl_code(0x22, 0, 0x800, 0); // METHOD_BUFFERED
const IOCTL_VSM_PUSH_AUDIO: u32 = ctl_code(0x22, 0, 0x801, 0);
const VSM_PROTOCOL_VERSION: u32 = 1;

const fn ctl_code(device_type: u32, access: u32, function: u32, method: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const INVALID_HANDLE_VALUE: isize = -1;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        lp_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security: *const core::ffi::c_void,
        creation: u32,
        flags: u32,
        template: *const core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
    fn DeviceIoControl(
        h: *mut core::ffi::c_void,
        code: u32,
        in_buf: *const core::ffi::c_void,
        in_size: u32,
        out_buf: *mut core::ffi::c_void,
        out_size: u32,
        returned: *mut u32,
        overlapped: *const core::ffi::c_void,
    ) -> i32;
    fn CloseHandle(h: *mut core::ffi::c_void) -> i32;
}

fn open_device() -> *mut core::ffi::c_void {
    let wide: Vec<u16> = DEVICE_PATH
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null(),
        )
    }
}

static STREAMING: AtomicBool = AtomicBool::new(false);
static CHANNEL: std::sync::Mutex<Option<rtrb::Producer<f32>>> = std::sync::Mutex::new(None);

/// True when the driver's control device exists. Cached briefly so per-frame
/// callers never hammer CreateFileW.
pub fn is_driver_present() -> bool {
    static CACHE: std::sync::Mutex<Option<(Instant, bool)>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((t, v)) = cache.as_ref() {
        if t.elapsed() < Duration::from_secs(2) {
            return *v;
        }
    }
    let h = open_device();
    let present = h as isize != INVALID_HANDLE_VALUE;
    if present {
        unsafe { CloseHandle(h) };
    }
    *cache = Some((Instant::now(), present));
    present
}

pub fn is_streaming() -> bool {
    STREAMING.load(Ordering::SeqCst)
}

/// Engine hook: called once per processed 10 ms frame.
/// First call opens the channel and spawns the writer thread (lazily — the
/// driver is normally absent, making this a no-op).
pub fn push_frame(frame: &[f32]) {
    let mut guard = CHANNEL.lock().unwrap();
    if guard.is_none() {
        if !is_driver_present() {
            return;
        }
        let (tx, rx) = RingBuffer::<f32>::new(9600);
        match std::thread::Builder::new()
            .name("vsmic-writer".into())
            .spawn(move || writer_loop(rx))
        {
            Ok(_) => {
                STREAMING.store(true, Ordering::SeqCst);
                log::info!("vsmic: streaming processed audio to {DEVICE_PATH}");
                *guard = Some(tx);
            }
            Err(_) => return,
        }
    }
    if let Some(tx) = guard.as_mut() {
        for &s in frame {
            if tx.push(s).is_err() {
                break; // writer stalled; drop rather than block the engine
            }
        }
    }
}

struct ChannelGuard;
impl Drop for ChannelGuard {
    fn drop(&mut self) {
        STREAMING.store(false, Ordering::SeqCst);
        log::info!("vsmic: writer stopped (driver gone?)");
    }
}

fn writer_loop(mut rx: rtrb::Consumer<f32>) -> ChannelGuard {
    let handle = open_device();
    if handle as isize == INVALID_HANDLE_VALUE {
        // driver vanished between check and open
        std::thread::sleep(Duration::from_millis(50));
        return ChannelGuard;
    }
    let mut version_out = 0u32;
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            IOCTL_VSM_GET_VERSION,
            std::ptr::null(),
            0,
            &mut version_out as *mut u32 as *mut _,
            4,
            &mut returned,
            std::ptr::null(),
        );
    }
    if returned == 4 && version_out != VSM_PROTOCOL_VERSION {
        log::error!("vsmic: driver protocol v{version_out} != app v{VSM_PROTOCOL_VERSION}");
        unsafe { CloseHandle(handle) };
        return ChannelGuard;
    }

    loop {
        // wait until a full frame is available (10 ms)
        while rx.slots() < 480 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut buf = [0f32; 480];
        for slot in buf.iter_mut() {
            *slot = rx.pop().unwrap_or(0.0);
        }
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_VSM_PUSH_AUDIO,
                buf.as_ptr() as *const _,
                (buf.len() * 4) as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null(),
            )
        };
        if ok == 0 {
            log::error!("vsmic: DeviceIoControl failed — stopping writer");
            unsafe { CloseHandle(handle) };
            return ChannelGuard;
        }
    }
}
