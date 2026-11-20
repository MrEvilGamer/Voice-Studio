# Voice Studio Virtual Microphone (our own kernel driver)

This is Voice Studio's own Windows virtual microphone — not a rebrand or
wrapper of anyone else's driver. It is a PortCLS/KMDF kernel driver based on
Microsoft's MIT-licensed SimpleAudioSample, modified so the microphone capture
buffer is fed by the AI Voice Studio app through a private control device.

## Architecture

```
Physical Mic
  → AI Voice Studio (WASAPI capture @ 48 kHz)
      → AGC → RNNoise → studio chain            (src-tauri/src: engine, dsp, ai)
      → vsmic writer thread                     (src-tauri/src/vsmic.rs)
          → IOCTL_VSM_PUSH_AUDIO                (10 ms frames, f32, 48 kHz mono)
              → \\.\VoiceStudioMic              (driver/vsvmic: vsmctrl.cpp ring)
                  → WaveRT capture buffer       (minwavertstream.cpp VsmReadSamples)
                      → "Voice Studio (Output)" recording endpoint
                          → Discord / FiveM / OBS
```

"Listen To Myself" is independent: the same processed stream also plays to the
selected monitor output through the normal WASAPI render path.

## Build

```
cd driver\vsvmic
build.cmd
```

Requires the WDK (winget: `Microsoft.WindowsWDK.10.0.26100`) — the script
drives cl.exe/link.exe directly, no Visual Studio or VSIX integration needed.
Output: `build\x64\VoiceStudioMic.sys`.

## Install (test-signing path)

Kernel drivers must be signed. For local development:

1. `signtool sign /v /fd sha256 /s PrivateCertStore /n "Voice Studio Test" build\x64\VoiceStudioMic.sys`
   (create a self-signed cert first with PowerShell `New-SelfSignedCertificate`)
2. Elevated:
   `pnputil /add-driver VoiceStudioMic.inf /install`
   `vsdevnode` (tools\vsdevnode.rs — builds with plain `rustc`; creates the
   ROOT\VoiceStudioMic device node so PnP binds the driver)
3. Enable test signing once, then reboot:
   `bcdedit /set testsigning on` — **requires Secure Boot disabled in BIOS**.
4. Windows shows **"Voice Studio (Output)"** as a microphone. AI Voice Studio
   detects the control device automatically and starts streaming.

## Public distribution path

Test-signing is not for end users. Shipping to others requires Microsoft
attestation signing: an EV code-signing certificate + Microsoft Partner Center
hardware submission (the .sys + INF go through the Hardware Dev Center dashboard
and come back WHQL/attestation-signed). After that the INF installs with no
test mode, no Secure Boot change.

## Status

- [x] Driver builds from source with the documented toolchain
- [x] Control device protocol implemented on both sides (Rust ⇄ C)
- [x] Capture path consumes the app's processed stream (no synthetic tones)
- [ ] Load testing on this machine (blocked by driver signing — see above)
- [ ] Attestation signing for public builds
