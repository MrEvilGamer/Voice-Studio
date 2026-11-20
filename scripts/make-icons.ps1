# Rebuild the approved Voice Studio emblem; the source lives in public/brand.
python "$PSScriptRoot\build-icons.py"
if ($LASTEXITCODE -ne 0) { throw 'Icon packaging failed' }
