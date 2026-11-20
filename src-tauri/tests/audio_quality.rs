use voice_studio::dsp::{preset, Agc, Biquad, GraphicEq, StudioChain};
use voice_studio::settings::EqConfig;
fn tone(hz: f32, amplitude: f32) -> Vec<f32> {
    (0..48000).map(|i| amplitude * (2.0 * std::f32::consts::PI * hz * i as f32 / 48000.0).sin()).collect()
}
fn rms(x: &[f32]) -> f32 { (x.iter().map(|v| v*v).sum::<f32>() / x.len() as f32).sqrt() }
#[test]
fn highpass_removes_rumble_without_harming_voice() {
    let mut filter=Biquad::identity(); filter.highpass(48000.0,80.0,0.707);
    let low:Vec<f32>=tone(25.0,0.5).into_iter().map(|v|filter.process(v)).collect();
    filter.reset();
    let mid:Vec<f32>=tone(1000.0,0.5).into_iter().map(|v|filter.process(v)).collect();
    let reference=0.5/2f32.sqrt();
    assert!(20.0*(reference/rms(&low[24000..])).log10()>15.0);
    assert!((20.0*(rms(&mid[24000..])/reference).log10()).abs()<0.5);
}
#[test]
fn deesser_unity_crossover_is_flat() {
    let mut params=preset("natural");
    params.mud_db=0.0; params.presence_db=0.0; params.air_db=0.0;
    params.deess_thresh_db=20.0; params.comp_ratio=1.0; params.makeup_db=0.0;
    let mut chain=StudioChain::new(48000.0,params);
    let mut signal=tone(6200.0,0.1); let before=rms(&signal[24000..]);
    chain.process(&mut signal);
    let delta=20.0*(rms(&signal[24000..])/before).log10();
    assert!(delta.abs()<0.1,"crossover gain was {delta} dB");
}
#[test]
fn disabling_auto_gain_returns_to_unity() {
    let mut gain=Agc::new(-18.0,true);
    for _ in 0..1000{gain.process(&mut[0.01;480]);}
    assert!(gain.gain_db()>10.0);
    gain.enabled=false;
    for _ in 0..1000{gain.process(&mut[0.01;480]);}
    assert!(gain.gain_db().abs()<0.05);
}
#[test]
fn bypassed_eq_preserves_signal() {
    let mut signal=tone(1000.0,0.1);let original=signal.clone();
    GraphicEq::new(48000.0).process(&mut signal,&EqConfig::default());
    assert!(signal.iter().zip(original).all(|(a,b)|(*a-b).abs()<1e-6));
}

fn eq_response(eq: &mut GraphicEq, config: &EqConfig, hz: f32) -> f32 {
    let mut energy = 0.0;
    let mut reference = 0.0;
    for block in 0..300 {
        let mut samples: Vec<f32> = (0..480).map(|i|
            0.02 * (2.0 * std::f32::consts::PI * hz * (block * 480 + i) as f32 / 48000.0).sin()
        ).collect();
        if block >= 200 { reference += samples.iter().map(|v| v*v).sum::<f32>(); }
        eq.process(&mut samples, config);
        if block >= 200 { energy += samples.iter().map(|v| v*v).sum::<f32>(); }
    }
    10.0 * (energy / reference).log10()
}

#[test]
fn every_equalizer_band_has_measured_response() {
    for (band, hz) in [31.,62.,125.,250.,500.,1000.,2000.,4000.,8000.,16000.].into_iter().enumerate() {
        let mut config = EqConfig::default(); config.enabled = true;
        config.gains[band] = -12.0;
        let cut = eq_response(&mut GraphicEq::new(48000.0), &config, hz);
        config.gains[band] = 12.0;
        let boost = eq_response(&mut GraphicEq::new(48000.0), &config, hz);
        println!("{hz} Hz: cut {cut:.2} dB; boost with automatic headroom {boost:.2} dB");
        assert!((cut + 12.0).abs() < 0.2, "{hz} Hz cut: {cut}");
        assert!(boost.abs() < 0.2, "{hz} Hz headroom-adjusted boost: {boost}");
    }
}

#[test]
fn equalizer_preamp_and_bypass_work_after_changes() {
    let mut eq = GraphicEq::new(48000.0);
    let mut config = EqConfig::default(); config.enabled = true; config.preamp = 6.0;
    let preamp = eq_response(&mut eq, &config, 1000.0);
    assert!((preamp - 6.0).abs() < 0.05);
    config.gains[5] = -12.0;
    assert!((eq_response(&mut eq, &config, 1000.0) + 6.0).abs() < 0.1);
    config.enabled = false;
    let bypass = eq_response(&mut eq, &config, 1000.0);
    println!("Preamp: {preamp:.2} dB; bypass after active EQ: {bypass:.2} dB");
    assert!(bypass.abs() < 0.05);
}
