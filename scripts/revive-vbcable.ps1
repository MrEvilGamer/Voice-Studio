$log = 'C:\Users\valen\Desktop\Voice Studio\scripts\revive-vbcable.log'
Start-Transcript -Path $log -Force
$ErrorActionPreference = 'Continue'

Write-Output '=== re-enabling VB-CABLE service (temporary dev fallback) ==='
sc.exe config VBAudioVACMME start= demand

Write-Output '=== restarting the device node so endpoints regenerate ==='
pnputil /disable-device "ROOT\MEDIA\0002"
Start-Sleep -Milliseconds 800
pnputil /enable-device "ROOT\MEDIA\0002"
Start-Sleep -Seconds 2

Write-Output '=== service state ==='
sc.exe query VBAudioVACMME | Select-String STATE
Stop-Transcript
