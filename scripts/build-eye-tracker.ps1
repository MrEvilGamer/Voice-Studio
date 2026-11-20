$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    python -m PyInstaller --noconfirm --onedir --console --name eye-worker --distpath src-tauri/tracker-runtime --workpath tracker/build --specpath tracker --collect-data mediapipe --collect-data customtkinter --exclude-module torch --exclude-module tensorflow --exclude-module jax --exclude-module IPython --exclude-module pandas --exclude-module scipy --exclude-module numba --exclude-module llvmlite --exclude-module pyarrow --exclude-module sklearn --exclude-module sympy --exclude-module bokeh --exclude-module seaborn --exclude-module plotly --exclude-module pytest --exclude-module notebook --exclude-module PyQt5 --exclude-module PyQt6 --exclude-module PySide2 --exclude-module PySide6 tracker/worker.py
    if ($LASTEXITCODE -ne 0) { throw 'Eye tracking runtime build failed.' }
    python scripts/bundle-tracker-licenses.py
    if ($LASTEXITCODE -ne 0) { throw 'Eye tracking notice packaging failed.' }
} finally { Pop-Location }

