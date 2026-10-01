# Drives the real GUI window through its tray behaviour and a mount, using the
# release build. Needs WinFsp and a free drive letter.
#   cargo build --release
#   powershell -File scripts\test-gui.ps1
#
# `--selftest` makes the app: minimise itself (it must vanish into the tray),
# be shown again from another thread (what the tray's "Show" does), hide on
# the close button instead of exiting, and, when an image is given with
# `--mount`, mount it, list the drive and unmount. Results go to a log file.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$exe = (Resolve-Path .\target\release\apfsreader-gui.exe).Path
$log = Join-Path $env:TEMP "apfsreader-gui-selftest.log"
$failures = 0

$cases = @(
    @{ label = "tray behaviour only"; args = @("--selftest") },
    @{ label = "APFS image";          args = @("--selftest", "--mount", "testdata\macos-fixtures\basic.dmg") },
    @{ label = "HFS+ image (LZMA)";   args = @("--selftest", "--mount", "testdata\macos-fixtures\hfs-basic-lzma.dmg") }
)
foreach ($c in $cases) {
    if ($c.args.Count -gt 2 -and -not (Test-Path $c.args[2])) { continue }
    Write-Host "== $($c.label)"
    if (Test-Path $log) { [IO.File]::Delete($log) }
    $argList = $c.args | ForEach-Object { if (Test-Path $_) { '"' + (Resolve-Path $_).Path + '"' } else { $_ } }
    $p = Start-Process -FilePath $exe -ArgumentList $argList -PassThru
    if (-not $p.WaitForExit(120000)) { $p.Kill(); Write-Host "  FAIL: timed out" -ForegroundColor Red; $failures++; continue }
    Get-Content $log | ForEach-Object {
        if ($_ -like "ok*") { Write-Host "  $_" -ForegroundColor Green }
        elseif ($_ -like "FAIL*") { Write-Host "  $_" -ForegroundColor Red }
        else { Write-Host "  $_" }
    }
    if ($p.ExitCode -ne 0) { $failures++ }
}
if ($failures -gt 0) { Write-Host "`n$failures case(s) failed" -ForegroundColor Red; exit 1 }
Write-Host "`nall GUI checks passed" -ForegroundColor Green
