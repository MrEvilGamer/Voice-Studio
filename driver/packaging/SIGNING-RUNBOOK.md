# Driver signing & distribution runbook (packaging requirement)

Users must never see Secure Boot, test mode, devcon, or INF screens. Signing is
handled here, once, in the release pipeline. The end-user experience is:
**install Voice Studio → "Voice Studio Microphone" appears in Windows.**

## One-time setup

1. **EV code-signing certificate** (Sectigo/DigiCert/GlobalSign, ~$300–500/yr,
   organization validation).
2. **Microsoft Partner Center** hardware account (register the EV cert at
   partner.microsoft.com → Hardware; one-time $99? — currently free for driver
   submission after dashboard access approval).

## Per release

1. `driver\packaging\make-submission.ps1` → builds `VoiceStudioMic-submission.cab`
2. Partner Center → Hardware → Submit new hardware:
   - upload the .cab, request **"attestation signing"** (no HLK tests needed
     for a software-only KS filter driver like ours)
   - Microsoft signs the driver with a Microsoft-issued certificate
   - download the signed package back
3. Verify the signed package installs on a clean machine (no test mode):
   `pnputil /add-driver VoiceStudioMic_signed.inf /install` + devnode tool
4. Ship the signed package **inside the Voice Studio NSIS installer**:
   - installer copies `VoiceStudioMic.{inf,sys,cat}` to `%ProgramData%\Voice Studio\driver`
   - runs elevated (installers already are):
     `pnputil /add-driver <path>\VoiceStudioMic.inf /install`
     `vsdevnode.exe` (creates ROOT\VoiceStudioMic so PnP binds it)
   - app starts, detects `\\.\VoiceStudioMic`, streams automatically

## Local development (maintainers only)

Test-signing on a dev machine requires Secure Boot off + `bcdedit /set
testsigning on` + reboot. Never required for end users.

## Current state

- [x] Driver source + build (driver\vsvmic, direct cl/link)
- [x] INF passes Microsoft signability validation (Inf2Cat, 10_X64)
- [x] Submission packaging script (make-submission.ps1)
- [x] Devnode creation tool (tools\vsdevnode.rs)
- [ ] EV cert + Partner Center submission (owner: business)
- [ ] Signed-package end-to-end install test
