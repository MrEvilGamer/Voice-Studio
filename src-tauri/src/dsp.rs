use std::f32::consts::PI;
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}
impl Biquad {
    pub const fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
    fn set(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        self.b0 = (b0 / a0) as f32;
        self.b1 = (b1 / a0) as f32;
        self.b2 = (b2 / a0) as f32;
        self.a1 = (a1 / a0) as f32;
        self.a2 = (a2 / a0) as f32;
    }
    fn base(fs: f32, f: f32) -> (f64, f64) {
        let w = 2.0 * std::f64::consts::PI * f.clamp(5.0, fs * 0.49) as f64 / fs as f64;
        (w.sin(), w.cos())
    }
    pub fn highpass(&mut self, fs: f32, f: f32, q: f32) {
        let (s, c) = Self::base(fs, f);
        let a = s / (2.0 * q as f64);
        self.set(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + a,
            -2.0 * c,
            1.0 - a,
        );
    }
    pub fn lowpass(&mut self, fs: f32, f: f32, q: f32) {
        let (s, c) = Self::base(fs, f);
        let a = s / (2.0 * q as f64);
        self.set(
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + a,
            -2.0 * c,
            1.0 - a,
        );
    }
    pub fn peaking(&mut self, fs: f32, f: f32, q: f32, g: f32) {
        let (s, c) = Self::base(fs, f);
        let a = 10f64.powf(g as f64 / 40.0);
        let k = s / (2.0 * q as f64);
        self.set(
            1.0 + k * a,
            -2.0 * c,
            1.0 - k * a,
            1.0 + k / a,
            -2.0 * c,
            1.0 - k / a,
        );
    }
    pub fn high_shelf(&mut self, fs: f32, f: f32, g: f32) {
        let (s, c) = Self::base(fs, f);
        let a = 10f64.powf(g as f64 / 40.0);
        let k = 2.0 * a.sqrt() * s / 2.0 * 2f64.sqrt();
        self.set(
            a * ((a + 1.0) + (a - 1.0) * c + k),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - k),
            (a + 1.0) - (a - 1.0) * c + k,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - k,
        );
    }
}
#[derive(Clone, Copy, Debug)]
pub struct StudioParams {
    pub hpf_hz: f32,
    pub mud_hz: f32,
    pub mud_db: f32,
    pub presence_hz: f32,
    pub presence_db: f32,
    pub air_hz: f32,
    pub air_db: f32,
    pub deess_thresh_db: f32,
    pub deess_max_db: f32,
    pub comp_thresh_db: f32,
    pub comp_ratio: f32,
    pub makeup_db: f32,
    pub agc_target_db: f32,
}
pub fn preset(name: &str) -> StudioParams {
    let mut p = StudioParams {
        hpf_hz: 80.0,
        mud_hz: 300.0,
        mud_db: -2.5,
        presence_hz: 3200.0,
        presence_db: 2.0,
        air_hz: 9500.0,
        air_db: 1.5,
        deess_thresh_db: -28.0,
        deess_max_db: 8.0,
        comp_thresh_db: -26.0,
        comp_ratio: 2.5,
        makeup_db: 4.0,
        agc_target_db: -18.0,
    };
    match name {
        "natural" => {
            p.hpf_hz = 70.0;
            p.mud_hz = 280.0;
            p.mud_db = -1.5;
            p.presence_hz = 3000.0;
            p.presence_db = 1.0;
            p.air_hz = 9000.0;
            p.air_db = 1.0;
            p.deess_thresh_db = -30.0;
            p.deess_max_db = 6.0;
            p.comp_thresh_db = -28.0;
            p.comp_ratio = 1.8;
            p.makeup_db = 2.0;
        }
        "gaming" => {
            p.hpf_hz = 100.0;
            p.mud_db = -3.0;
            p.comp_thresh_db = -24.0;
            p.comp_ratio = 3.0;
            p.makeup_db = 4.0;
            p.agc_target_db = -16.0;
        }
        "broadcast" => {
            p.mud_hz = 250.0;
            p.mud_db = -3.5;
            p.presence_hz = 3300.0;
            p.presence_db = 2.5;
            p.air_hz = 10000.0;
            p.comp_ratio = 3.5;
            p.makeup_db = 5.0;
            p.agc_target_db = -16.0;
        }
        "podcast" => {
            p.air_db = 0.5;
            p.comp_ratio = 3.0;
            p.presence_db = 1.5;
            p.makeup_db = 4.0;
        }
        "warm" => {
            p.hpf_hz = 60.0;
            p.mud_db = 1.0;
            p.presence_db = 0.5;
            p.air_db = -1.0;
            p.makeup_db = 2.0;
        }
        "clarity" => {
            p.mud_db = -4.0;
            p.presence_db = 3.0;
            p.air_db = 2.0;
            p.makeup_db = 3.0;
        }
        "meeting" => {
            p.hpf_hz = 120.0;
            p.air_db = 0.0;
            p.presence_db = 2.5;
            p.comp_ratio = 3.0;
            p.makeup_db = 3.0;
        }
        _ => {}
    }
    p
}
pub struct Agc {
    pub enabled: bool,
    gain: f32,
    target_db: f32,
}
impl Agc {
    pub fn new(target_db: f32, enabled: bool) -> Self {
        Self {
            enabled,
            gain: 1.0,
            target_db,
        }
    }
    pub fn set_target(&mut self, db: f32) {
        self.target_db = db;
    }
    pub fn gain_db(&self) -> f32 {
        20.0 * self.gain.max(1e-6).log10()
    }
    pub fn process(&mut self, x: &mut [f32]) {
        if x.is_empty() {
            return;
        }
        let n = x.len() as f32;
        let rms = (x.iter().map(|v| v * v).sum::<f32>() / n).sqrt().max(1e-9);
        let db = 20.0 * rms.log10();
        let cur = self.gain_db();
        let target = if !self.enabled {
            0.0
        } else if db < -55.0 {
            cur.min(0.0)
        } else {
            (self.target_db - db).clamp(-6.0, 18.0)
        };
        let g = 10f32.powf((cur + (target - cur).clamp(-1.5, 1.5) * 0.06) / 20.0);
        let step = (g - self.gain) / n;
        for v in x {
            *v *= self.gain;
            self.gain += step;
        }
        self.gain = g;
    }
}
pub struct Limiter {
    env: f32,
    ceiling: f32,
    release: f32,
}
impl Limiter {
    pub fn new(fs: f32, db: f32) -> Self {
        Self {
            env: 0.0,
            ceiling: 10f32.powf(db / 20.0),
            release: 1.0 - (-1.0 / (0.08 * fs)).exp(),
        }
    }
    pub fn process(&mut self, x: f32) -> f32 {
        if !x.is_finite() {
            return 0.0;
        }
        let a = x.abs();
        self.env = if a > self.env {
            a
        } else {
            self.env + self.release * (a - self.env)
        };
        let g = if self.env > self.ceiling {
            self.ceiling / self.env
        } else {
            1.0
        };
        (x * g).clamp(-0.999, 0.999)
    }
}
pub struct StudioChain {
    fs: f32,
    params: StudioParams,
    hpf: Biquad,
    mud: Biquad,
    presence: Biquad,
    air: Biquad,
    lp: [Biquad; 2],
    hp: [Biquad; 2],
    de_env: f32,
    de_gr: f32,
    env: f32,
    gr: f32,
    lim: Limiter,
}
impl StudioChain {
    pub fn new(fs: f32, params: StudioParams) -> Self {
        let mut c = Self {
            fs,
            params,
            hpf: Biquad::identity(),
            mud: Biquad::identity(),
            presence: Biquad::identity(),
            air: Biquad::identity(),
            lp: [Biquad::identity(); 2],
            hp: [Biquad::identity(); 2],
            de_env: 0.0,
            de_gr: 0.0,
            env: 0.0,
            gr: 0.0,
            lim: Limiter::new(fs, -1.5),
        };
        c.set_params(params);
        c
    }
    pub fn params(&self) -> StudioParams {
        self.params
    }
    pub fn agc_target_db(&self) -> f32 {
        self.params.agc_target_db
    }
    pub fn set_params(&mut self, p: StudioParams) {
        self.params = p;
        self.hpf.highpass(self.fs, p.hpf_hz, 0.707);
        self.mud.peaking(self.fs, p.mud_hz, 0.8, p.mud_db);
        self.presence
            .peaking(self.fs, p.presence_hz, 0.9, p.presence_db);
        self.air.high_shelf(self.fs, p.air_hz, p.air_db);
        for f in &mut self.lp {
            f.lowpass(self.fs, 6200.0, 0.707);
        }
        for f in &mut self.hp {
            f.highpass(self.fs, 6200.0, 0.707);
        }
    }
    pub fn process(&mut self, x: &mut [f32]) {
        let p = self.params;
        let coef = |sec: f32| 1.0 - (-1.0 / (sec * self.fs)).exp();
        let att = coef(0.008);
        let rel = coef(0.15);
        for v in x {
            let y = self.air.process(
                self.presence
                    .process(self.mud.process(self.hpf.process(*v))),
            );
            let low = self.lp[0].process(y);
            let low = self.lp[1].process(low);
            let high = self.hp[0].process(y);
            let high = self.hp[1].process(high);
            let e = high * high;
            self.de_env += if e > self.de_env {
                coef(0.003)
            } else {
                coef(0.03)
            } * (e - self.de_env);
            let dg = ((10.0 * self.de_env.max(1e-12).log10() - p.deess_thresh_db) * (2.0 / 3.0))
                .clamp(0.0, p.deess_max_db);
            self.de_gr += if dg > self.de_gr {
                coef(0.0005)
            } else {
                coef(0.04)
            } * (dg - self.de_gr);
            let y = low + high * 10f32.powf(-self.de_gr / 20.0);
            let e = y * y;
            self.env += if e > self.env { att } else { rel } * (e - self.env);
            let over = 10.0 * self.env.max(1e-12).log10() - p.comp_thresh_db;
            let target = if over < -3.0 {
                0.0
            } else if over > 3.0 {
                over * (1.0 - 1.0 / p.comp_ratio)
            } else {
                (over + 3.0).powi(2) / 12.0 * (1.0 - 1.0 / p.comp_ratio)
            };
            self.gr += if target > self.gr { att } else { rel } * (target - self.gr);
            *v = self
                .lim
                .process(y * 10f32.powf((p.makeup_db - self.gr) / 20.0));
        }
    }
}
pub struct GraphicEq {
    filters: [Biquad; 10],
    gains: [f32; 10],
    gain: f32,
    fs: f32,
}
impl GraphicEq {
    pub fn new(fs: f32) -> Self {
        Self {
            filters: [Biquad::identity(); 10],
            gains: [0.0; 10],
            gain: 1.0,
            fs,
        }
    }
    pub fn process(&mut self, data: &mut [f32], config: &crate::settings::EqConfig) {
        let freq = [
            31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
        ];
        let mut boost = 0.0f32;
        for i in 0..10 {
            let target = if config.enabled { config.gains[i] } else { 0.0 };
            self.gains[i] += (target - self.gains[i]) * 0.12;
            boost = boost.max(self.gains[i]);
            self.filters[i].peaking(self.fs, freq[i], 1.4, self.gains[i]);
        }
        let target = 10f32.powf(
            (if config.enabled {
                config.preamp - boost
            } else {
                0.0
            }) / 20.0,
        );
        let step = (target - self.gain) / data.len().max(1) as f32;
        for x in data {
            self.gain += step;
            let mut y = *x * self.gain;
            for f in &mut self.filters {
                y = f.process(y);
            }
            *x = if y.is_finite() {
                y.clamp(-0.999, 0.999)
            } else {
                0.0
            };
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silence_does_not_raise_gain() {
        let mut a = Agc::new(-18.0, true);
        for _ in 0..1000 {
            a.process(&mut [0.0; 480]);
        }
        assert!(a.gain_db().abs() < 0.01);
    }
    #[test]
    fn limiter_bounds_signal() {
        let mut l = Limiter::new(48000.0, -1.5);
        for i in 0..48000 {
            let x = 4.0 * (i as f32 * PI / 32.0).sin();
            assert!(l.process(x).abs() <= 10f32.powf(-1.5 / 20.0) + 0.0001);
        }
    }
    #[test]
    fn eq_boosts_target_frequency() {
        let mut eq = GraphicEq::new(48000.0);
        let mut c = crate::settings::EqConfig::default();
        c.enabled = true;
        c.gains[5] = 6.0;
        let mut energy = 0.0;
        for b in 0..100 {
            let mut x: Vec<f32> = (0..480)
                .map(|i| 0.1 * (2.0 * PI * 1000.0 * (b * 480 + i) as f32 / 48000.0).sin())
                .collect();
            eq.process(&mut x, &c);
            if b > 50 {
                energy += x.iter().map(|v| v * v).sum::<f32>();
            }
        }
        assert!(energy.is_finite() && energy > 50.0);
    }
    #[test]
    fn every_preset_is_finite() {
        for name in [
            "natural",
            "studio",
            "gaming",
            "broadcast",
            "podcast",
            "warm",
            "clarity",
            "meeting",
        ] {
            let mut c = StudioChain::new(48000.0, preset(name));
            let mut x: Vec<f32> = (0..48000)
                .map(|i| 0.3 * (i as f32 * PI / 24.0).sin())
                .collect();
            c.process(&mut x);
            assert!(x.iter().all(|v| v.is_finite() && v.abs() < 1.0));
        }
    }
}
