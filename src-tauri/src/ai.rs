//! RNNoise real-time noise suppression (vendored xiph C sources, compiled by build.rs).
//! The FFI surface is intentionally tiny: create / process / destroy.

#[repr(C)]
pub struct DenoiseState {
    _opaque: [u8; 0],
}
#[repr(C)]
pub struct RnnModel {
    _opaque: [u8; 0],
}

extern "C" {
    fn rnnoise_get_frame_size() -> i32;
    fn rnnoise_create(model: *mut RnnModel) -> *mut DenoiseState;
    fn rnnoise_destroy(st: *mut DenoiseState);
    /// Processes one frame in place. Returns VAD speech probability 0..1.
    fn rnnoise_process_frame(st: *mut DenoiseState, out: *mut f32, input: *const f32) -> f32;
}

pub struct Denoiser {
    st: *mut DenoiseState,
    frame_size: usize,
}

// The raw state is moved between threads only behind &mut usage; RNNoise state
// is self-contained (no globals) and we only ever touch it from one thread.
unsafe impl Send for Denoiser {}

impl Denoiser {
    pub fn new() -> Result<Self, String> {
        let st = unsafe { rnnoise_create(std::ptr::null_mut()) };
        if st.is_null() {
            return Err("RNNoise: failed to allocate denoise state".to_string());
        }
        let frame_size = unsafe { rnnoise_get_frame_size() } as usize;
        if frame_size == 0 {
            unsafe { rnnoise_destroy(st) };
            return Err("RNNoise: invalid frame size".to_string());
        }
        Ok(Self { st, frame_size })
    }

    pub fn frame_size(&self) -> usize {
        self.frame_size
    }

    /// Processes `frame` in place (len must equal frame_size).
    /// Returns the VAD speech probability.
    ///
    /// RNNoise is trained on PCM16-scale samples (±32768), so the wrapper
    /// scales up before the call and back down afterwards.
    #[must_use]
    pub fn process(&mut self, frame: &mut [f32]) -> f32 {
        debug_assert_eq!(frame.len(), self.frame_size);
        if frame.len() != self.frame_size {
            return 0.0;
        }
        for s in frame.iter_mut() {
            *s *= 32768.0;
        }
        let vad = unsafe { rnnoise_process_frame(self.st, frame.as_mut_ptr(), frame.as_ptr()) };
        for s in frame.iter_mut() {
            *s /= 32768.0;
        }
        vad
    }
}

impl Drop for Denoiser {
    fn drop(&mut self) {
        unsafe { rnnoise_destroy(self.st) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_stationary_noise() {
        let mut d = Denoiser::new().unwrap();
        let fs = 48000.0f32;
        let n = d.frame_size();
        assert_eq!(
            n, 480,
            "RNNoise must be built for 48 kHz / 480-sample frames"
        );

        // 0.5 s voice (lets VAD lock), then 1.5 s pure stationary noise.
        // Measure suppression only over the noise-only tail.
        let mut input_noise_db = 0.0f32;
        let mut output_noise_db = 0.0f32;
        let mut state = 0f32;
        let frames = 200; // 2 s
        for f in 0..frames {
            let mut buf = vec![0f32; n];
            for i in 0..n {
                let t = (f * n + i) as f32 / fs;
                state = (state * 16807.0 + 0.317).sin() * 43758.5453;
                let noise = state.fract() * 2.0 - 1.0;
                let voice = if t < 0.5 {
                    (2.0 * std::f32::consts::PI * 1000.0 * t).sin() * 0.2
                } else {
                    0.0
                };
                buf[i] = voice + noise * 0.05;
            }
            let in_rms = (buf.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
            d.process(&mut buf);
            let out_rms = (buf.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
            if f >= 60 {
                // noise-only region, past voice end + convergence
                input_noise_db += in_rms * in_rms;
                output_noise_db += out_rms * out_rms;
            }
        }
        let red_db = 10.0 * (input_noise_db / output_noise_db.max(1e-20)).log10();
        assert!(
            red_db > 8.0,
            "expected >8 dB noise reduction in noise-only segment, got {:.1} dB",
            red_db
        );
    }
}
