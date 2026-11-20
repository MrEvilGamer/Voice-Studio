# Produces the driver submission package for Microsoft attestation signing.
# Result: dist\VoiceStudioMic-submission.cab  -> upload to Partner Center
#         (Hardware Dev Center -> Drivers -> Submit new hardware)
#
# Prereqs: WDK (winget Microsoft.WindowsWDK.10.0.26100), driver built (vsvmic\build.cmd)
# After Microsoft signs it: download the signed package, embed it in the
# Voice Studio installer, which installs it elevated during setup (standard
# driver bundling, exactly what NVIDIA Broadcast / Voicemod installers do).

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot          # driver/
$vsvmic = Join-Path $root 'vsvmic'
$dist = Join-Path $PSScriptRoot 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null

# 1. build + catalog
cmd /c (Join-Path $vsvmic 'build.cmd') | Out-Null
cmd /c (Join-Path $vsvmic 'makecat.cmd') | Out-Null

# 2. stage the submission set (inf + sys + cat at package root)
$stage = Join-Path $dist 'package'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item (Join-Path $vsvmic 'VoiceStudioMic.inf') $stage
Copy-Item (Join-Path $vsvmic 'VoiceStudioMic.sys') $stage
Copy-Item (Join-Path $vsvmic 'VoiceStudioMic.cat') $stage

# 3. cab it (Partner Center wants a single .cab)
$ddf = Join-Path $dist 'submission.ddf'
@"
.Set CabinetNameTemplate="VoiceStudioMic-submission.cab"
.Set DiskDirectory1="$dist"
.Set CompressionType=LZX
.Set Cabinet=ON
.Set InfFileHeader=.inf
"$stage\VoiceStudioMic.inf"
"$stage\VoiceStudioMic.sys"
"$stage\VoiceStudioMic.cat"
"@ | Set-Content $ddf
makecab /f $ddf

Write-Host "Submission package: $dist\VoiceStudioMic-submission.cab"
