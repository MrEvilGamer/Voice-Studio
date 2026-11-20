$log = 'C:\Users\valen\Desktop\Voice Studio\scripts\uninstall-vbcable.log'
Start-Transcript -Path $log -Force
$ErrorActionPreference = 'Continue'

Write-Output '=== removing VB-CABLE driver package ==='
$blocks = (pnputil /enum-drivers | Out-String) -split "\r?\n\r?\n"
foreach ($b in $blocks) {
    if ($b -match 'vbmme|vb-audio|vbaudio') {
        $pub = ([regex]::Match($b, '(?i)Published Name\s*:\s*(oem\d+\.inf)')).Groups[1].Value
        if ($pub) {
            Write-Output "deleting $pub"
            pnputil /delete-driver $pub /uninstall /force
        }
    }
}

Write-Output '=== removing leftover service ==='
sc.exe delete VBAudioVACMME

Write-Output '=== audio devices after removal ==='
Get-CimInstance Win32_PnPEntity | Where-Object { $_.Name -like '*CABLE*' } | ForEach-Object { Write-Output $_.Name }
Write-Output '(empty above = VB-CABLE fully gone)'

Write-Output '=== driver load prerequisites for our own driver ==='
try {
    $sb = Confirm-SecureBootUEFI
    Write-Output "SecureBoot=$sb"
} catch {
    Write-Output "SecureBoot query failed: $($_.Exception.Message)"
}
$ts = bcdedit /enum '{current}' | Select-String -Pattern 'testsigning'
Write-Output "testsigning: $ts"
Stop-Transcript
