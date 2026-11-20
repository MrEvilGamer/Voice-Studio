# Integrated Eye Tracker

Imported from the user's local Eye Tracker project (OpenGaze). `core.py` retains
the original face/iris tracking, filtering, calibration and OpenTrack UDP logic.
`worker.py` bridges it to the Voice Studio React interface over local JSON pipes;
the former main window is hidden and only the overlay/calibration surfaces appear.

The camera starts on demand. Mouse control is disabled on every fresh launch.
Overlay and game output start disabled for new installations. Pausing or losing
the face stops cursor updates and hides the overlay. F8 recenters, F9 pauses,
and F10 toggles mouse control while tracking is running.

Settings and personal calibration live in `%APPDATA%/Voice Studio/Eye Tracker`,
not in the installation directory. The current user's calibration was migrated
from the source project; personal calibration is not bundled in the installer.
Original source files remain in place.

Build the runtime after installing `tracker/requirements.txt`:
`powershell -File scripts/build-eye-tracker.ps1`, then `npm run tauri build`.
The installer includes the Python runtime, models, native dependencies and their
bundled license files. Run `python scripts/check-eye-tracker.py` for a packaged
worker lifecycle check; user calibration accuracy must be assessed interactively.

## Preview transport

The camera capture thread publishes one latest raw frame. A separate JPEG worker
streams preview events at up to 30 fps, independently of face inference. The
frontend displays the newest event on animation frames, discards events older
than 400 ms, and avoids rerendering the settings panel for every image. Metadata
polling carries no image payload. Leaving the Eye Tracking page stops preview
encoding while tracking can continue. Camera teardown belongs to the capture
thread; the dedicated worker saves settings before exiting on Stop.

Measure the camera stream using the current camera profile without changing user
settings or enabling pointer/game output:
`python scripts/check-eye-tracker.py --profile-current` (bundled) or add
`--source` to test source code. The check reports delivered frames and
capture-to-IPC frame age, tests preview disable, and checks shutdown/persistence.
