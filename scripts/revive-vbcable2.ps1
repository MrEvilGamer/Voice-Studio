$log = 'C:\Users\valen\Desktop\Voice Studio\scripts\revive-vbcable2.log'
Start-Transcript -Path $log -Force
$ErrorActionPreference = 'Continue'

Write-Output '=== purging the zombie VB-CABLE state ==='
pnputil /remove-device "ROOT\MEDIA\0002"
sc.exe stop VBAudioVACMME
reg delete "HKLM\SYSTEM\CurrentControlSet\Services\VBAudioVACMME" /f

Write-Output '=== fresh install: click Install Driver in the setup window ==='
$setup = "$env:TEMP\vbcable\VBCABLE_Setup_x64.exe"
if (Test-Path $setup) {
    Start-Process $setup -WorkingDirectory "$env:TEMP\vbcable"
} else {
    Write-Output "SETUP MISSING at $setup"
}

Write-Output '=== waiting up to 120s for endpoints ==='
$base = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio'
$desc = '{a45c254e-df1c-4efd-8020-67d146a850e0},2'
$found = $false
foreach ($i in 1..60) {
    Start-Sleep -Seconds 2
    $hit = $false
    foreach ($kind in 'Capture','Render') {
        foreach ($k in Get-ChildItem (Join-Path $base $kind) -ErrorAction SilentlyContinue) {
            $p = Get-ItemProperty (Join-Path $k.PSPath 'Properties') -ErrorAction SilentlyContinue
            if ($p.$desc -like '*CABLE*') { $hit = $true; Write-Output "FOUND: $kind :: $($p.$desc)" }
        }
    }
    if ($hit) { $found = $true; break }
}
Write-Output "RESULT: endpoints_found=$found"
Stop-Transcript
