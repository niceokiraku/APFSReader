# End-to-end check of apfsreader-mount through real Windows file APIs.
# Needs WinFsp installed and a free drive letter. Run from the repository root:
#   cargo build --release
#   powershell -File scripts\test-mount.ps1 [-Letter R]
#
# For each fixture it mounts the image, then verifies every file against the
# SHA-256 manifest recorded when macOS made the image, checks that writes are
# refused, lists folders larger than one directory-read buffer (the case that
# once looped forever), and confirms the drive disappears on unmount.
param([string]$Letter = "R")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$exe = (Resolve-Path .\target\release\apfsreader-mount.exe).Path
$drive = "${Letter}:"
$failures = 0
function Fail($msg) { Write-Host "  FAIL: $msg" -ForegroundColor Red; $script:failures++ }
function Pass($msg) { Write-Host "  ok:   $msg" -ForegroundColor Green }

function With-Mount($image, [scriptblock]$body) {
    if (Test-Path "$drive\") { throw "$drive is already in use; pass -Letter with a free letter" }
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe
    $psi.Arguments = "`"$((Resolve-Path $image).Path)`" $drive"
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    try {
        $deadline = (Get-Date).AddSeconds(15)
        while (-not (Test-Path "$drive\") -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
        if (-not (Test-Path "$drive\")) { throw "mount did not appear: $($p.StandardError.ReadToEnd())" }
        & $body
    } finally {
        $p.StandardInput.WriteLine("")
        if (-not $p.WaitForExit(10000)) { $p.Kill(); Fail "mount did not exit after Enter" }
        Start-Sleep -Milliseconds 500
        if (Test-Path "$drive\") { Fail "$drive still present after unmount" } else { Pass "unmounted cleanly" }
    }
}

function Check-Manifest($manifestFile) {
    $m = Get-Content $manifestFile -Raw -Encoding UTF8 | ConvertFrom-Json
    $ok = 0; $bad = 0
    foreach ($prop in $m.files.PSObject.Properties) {
        $info = $prop.Value
        if ($info.type -ne 'file') { continue }
        $p = "$drive\" + ($prop.Name -replace '/', '\')
        if ((Test-Path -LiteralPath $p) -and ((Get-FileHash -LiteralPath $p).Hash.ToLower() -eq $info.sha256) -and
            ((Get-Item -LiteralPath $p).Length -eq $info.size)) { $ok++ } else { Fail "$($prop.Name)"; $bad++ }
    }
    if ($bad -eq 0 -and $ok -ge 9) { Pass "$ok files match the manifest" }
}

function Check-ReadOnly {
    try { Set-Content -LiteralPath "$drive\should-not-exist.txt" "x" -ErrorAction Stop; Fail "a write succeeded" }
    catch { Pass "writes are refused" }
    if (Test-Path "$drive\should-not-exist.txt") { Fail "a file was created" }
}

function Check-BigFolder($path, $expected) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $job = Start-Job -ArgumentList $path { param($p) @(Get-ChildItem -LiteralPath $p).Count }
    if (-not (Wait-Job $job -Timeout 30)) { Stop-Job $job; Fail "listing $path did not finish in 30 s (endless directory enumeration?)"; return }
    $n = Receive-Job $job
    if ($n -eq $expected) { Pass ("{0} has {1} entries, listed in {2:N2} s" -f $path, $n, $sw.Elapsed.TotalSeconds) }
    else { Fail "$path listed $n entries, expected $expected" }
}

$fixtures = @(
    @{ image = "testdata\macos-fixtures\basic.dmg";         manifest = "testdata\macos-fixtures\manifest.json";     label = "APFS, zlib DMG" },
    @{ image = "testdata\macos-fixtures\basic-lzfse.dmg";   manifest = "testdata\macos-fixtures\manifest.json";     label = "APFS, LZFSE DMG" },
    @{ image = "testdata\macos-fixtures\hfs-basic.dmg";     manifest = "testdata\macos-fixtures\hfs-manifest.json"; label = "HFS+, zlib DMG" },
    @{ image = "testdata\macos-fixtures\hfs-basic-lzma.dmg"; manifest = "testdata\macos-fixtures\hfs-manifest.json"; label = "HFS+, LZMA DMG" }
)
foreach ($f in $fixtures) {
    if (-not (Test-Path $f.image)) { continue }
    Write-Host "== $($f.label)"
    With-Mount $f.image { Check-Manifest $f.manifest; Check-ReadOnly }
}

# Unmounting must release the image file, so it can be deleted afterwards.
foreach ($f in @(@{ src = "testdata\macos-fixtures\basic.dmg"; label = "APFS" }, @{ src = "testdata\macos-fixtures\hfs-basic.dmg"; label = "HFS+" })) {
    if (-not (Test-Path $f.src)) { continue }
    Write-Host "== image handle released after unmount, $($f.label)"
    $copy = Join-Path $env:TEMP ("apfsreader-release-test-{0}.dmg" -f [Guid]::NewGuid().ToString("N"))
    Copy-Item $f.src $copy
    With-Mount $copy { Pass "mounted a private copy" }
    try { [IO.File]::Delete($copy); if (Test-Path $copy) { Fail "the copy is still there" } else { Pass "the image file could be deleted (handle released)" } }
    catch { Fail "the image file is still locked after unmount: $($_.Exception.Message)" }
}

foreach ($big in @(@{ image = "testdata\gen\big-apfs.dmg"; label = "APFS" }, @{ image = "testdata\gen\big-hfsplus.dmg"; label = "HFS+" })) {
    if (-not (Test-Path $big.image)) { continue }
    Write-Host "== large folders, $($big.label)"
    With-Mount $big.image {
        Check-BigFolder "$drive\flat" 8000
        Check-BigFolder "$drive\nest\d7" 100
        $t = (Measure-Command { 1..200 | ForEach-Object { [void](Get-Content -LiteralPath "$drive\flat\f$($_ * 37).txt") } }).TotalSeconds
        if ($t -lt 20) { Pass ("200 file reads in {0:N2} s" -f $t) } else { Fail "200 file reads took $t s" }
    }
}

if ($failures -gt 0) { Write-Host "`n$failures check(s) failed" -ForegroundColor Red; exit 1 }
Write-Host "`nall mount checks passed" -ForegroundColor Green
