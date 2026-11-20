fn main() {
    // Compile vendored xiph RNNoise (C) — no bindgen, plain cc.
    let mut build = cc::Build::new();
    build
        .include("vendor/rnnoise-master/src")
        .include("vendor/rnnoise-master/include")
        .file("vendor/rnnoise-master/src/denoise.c")
        .file("vendor/rnnoise-master/src/rnn.c")
        .file("vendor/rnnoise-master/src/rnn_data.c")
        .file("vendor/rnnoise-master/src/pitch.c")
        .file("vendor/rnnoise-master/src/celt_lpc.c")
        .file("vendor/rnnoise-master/src/kiss_fft.c")
        .define("RNNOISE_BUILD", None);
    // MSVC needs this for M_PI
    build.define("_USE_MATH_DEFINES", None);
    let probe = build.get_compiler();
    if probe.is_like_msvc() {
        build.std("c11");
    } else {
        build.std("c99");
    }
    build.warnings(false).compile("rnnoise");
    println!("cargo:rerun-if-changed=vendor/rnnoise-master");

    tauri_build::build();
}
