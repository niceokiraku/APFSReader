# Builds everything that is distributed, and checks it.
#   powershell -File scripts\release.ps1 [-SkipTests] [-SkipSmokeTest]
#
# Produces in dist\ :
#   APFSReader-Setup-<version>.exe          the installer (Inno Setup)
#   APFSReader-<version>-win64.zip          portable ZIP
#   APFSReader-<version>-source.zip         corresponding source (the GPL parts require it)
#   SHA256SUMS.txt
#
# Checks before it finishes: no build-machine path or user name inside any program,
# every program has the app icon and the right version, the installer installs,
# starts and uninstalls cleanly (a per-user install in a temporary folder).
param([switch]$SkipTests, [switch]$SkipSmokeTest)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$env:Path += ";$env:USERPROFILE\.cargo\bin"

function Step($m) { Write-Host "`n== $m" -ForegroundColor Cyan }
function Fail($m) { Write-Host "FAIL: $m" -ForegroundColor Red; exit 1 }

# ---- version ----------------------------------------------------------------
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
Write-Host "APFSReader $version"

# ---- build ------------------------------------------------------------------
Step "build (build-machine paths are removed from the binaries)"
# --remap-path-prefix rewrites the paths rustc embeds (panic messages, debug info),
# so neither the user name nor the project folder ends up in a distributed file.
$remaps = @("--remap-path-prefix=$env:USERPROFILE=C:\home", "--remap-path-prefix=$root=src")
$env:RUSTFLAGS = $remaps -join " "
if (-not $SkipTests) {
    cargo test --workspace
    if ($LASTEXITCODE -ne 0) { Fail "tests failed" }
}
cargo build --release --workspace
if ($LASTEXITCODE -ne 0) { Fail "release build failed" }
$env:RUSTFLAGS = $null

# ---- licences ---------------------------------------------------------------
Step "third-party licences"
powershell -NoProfile -File "$root\scripts\gen-third-party.ps1" | Select-Object -First 1

# ---- stage ------------------------------------------------------------------
Step "stage"
$dist = Join-Path $root "dist"
$stage = Join-Path $dist "stage"
if (Test-Path $stage) { [IO.Directory]::Delete($stage, $true) }
New-Item -ItemType Directory -Force $stage | Out-Null
$programs = "apfsreader-gui.exe", "apfsreader-broker.exe", "apfsreader.exe", "apfsreader-mount.exe"
foreach ($p in $programs) { Copy-Item "$root\target\release\$p" $stage }
foreach ($f in "LICENSE.md", "LICENSE-MIT", "COPYING", "THIRD_PARTY_LICENSES.md") { Copy-Item "$root\$f" $stage }
Copy-Item "$root\docs\USER_GUIDE.ja.md", "$root\docs\USER_GUIDE.en.md" $stage
cargo run -q -p apfsreader-icon --example make_ico -- "$stage\apfsreader.ico"
if ($LASTEXITCODE -ne 0) { Fail "could not write the icon" }

# ---- inspect the programs ---------------------------------------------------
Step "inspect the programs"
Add-Type -AssemblyName System.Drawing
$userName = $env:USERNAME
foreach ($p in $programs) {
    $path = Join-Path $stage $p
    $bytes = [IO.File]::ReadAllBytes($path)
    $ascii = [Text.Encoding]::ASCII.GetString($bytes)
    $utf16 = [Text.Encoding]::Unicode.GetString($bytes)
    # The user name alone is too short a needle (it may be an ordinary word); look for it as a path.
    foreach ($needle in @("Users\$userName", "Users/$userName", $root, $env:USERPROFILE)) {
        if ($needle -and ($ascii.IndexOf($needle, [StringComparison]::OrdinalIgnoreCase) -ge 0 -or $utf16.IndexOf($needle, [StringComparison]::OrdinalIgnoreCase) -ge 0)) {
            Fail "$p contains '$needle' (a build-machine path or the user name)"
        }
    }
    $vi = (Get-Item $path).VersionInfo
    if ($vi.ProductName -ne "APFSReader") { Fail "$p has product name '$($vi.ProductName)'" }
    if ($vi.FileVersion -notlike "$version*") { Fail "$p has file version '$($vi.FileVersion)', expected $version" }
    $icon = [System.Drawing.Icon]::ExtractAssociatedIcon($path)
    $bmp = $icon.ToBitmap(); $c = $bmp.GetPixel(16, 16)
    if ($c.A -lt 200) { Fail "$p has no app icon" }
    Write-Host ("  ok  {0,-24} {1,6:N1} KB  version {2}" -f $p, ($bytes.Length / 1KB), $vi.FileVersion)
}

# ---- packages ---------------------------------------------------------------
Step "portable ZIP"
$zipName = "APFSReader-$version-win64.zip"
$zip = Join-Path $dist $zipName
if (Test-Path $zip) { [IO.File]::Delete($zip) }
$zipRoot = Join-Path $dist "zip-$version"
if (Test-Path $zipRoot) { [IO.Directory]::Delete($zipRoot, $true) }
$inner = Join-Path $zipRoot "APFSReader-$version"
New-Item -ItemType Directory -Force $inner | Out-Null
Get-ChildItem $stage -File | Where-Object { $_.Name -ne "apfsreader.ico" } | Copy-Item -Destination $inner
Compress-Archive -Path $inner -DestinationPath $zip
[IO.Directory]::Delete($zipRoot, $true)

Step "source ZIP"
$srcName = "APFSReader-$version-source.zip"
$srcZip = Join-Path $dist $srcName
if (Test-Path $srcZip) { [IO.File]::Delete($srcZip) }
$srcRoot = Join-Path $dist "src-$version"
if (Test-Path $srcRoot) { [IO.Directory]::Delete($srcRoot, $true) }
$srcInner = Join-Path $srcRoot "APFSReader-$version-source"
New-Item -ItemType Directory -Force $srcInner | Out-Null
foreach ($d in "core", "cli", "mount", "broker", "gui", "icon", "build-resources", "installer", "scripts", "docs", ".cargo") {
    robocopy "$root\$d" "$srcInner\$d" /E /XD target /XF *.png /NFL /NDL /NJH /NJS /NP | Out-Null
}
foreach ($f in "Cargo.toml", "Cargo.lock", "README.md", "LICENSE.md", "LICENSE-MIT", "COPYING", "THIRD_PARTY_LICENSES.md", ".gitignore") {
    if (Test-Path "$root\$f") { Copy-Item "$root\$f" $srcInner }
}
# Test fixtures come from other projects and are not redistributed; scripts say how to get them.
Compress-Archive -Path $srcInner -DestinationPath $srcZip
[IO.Directory]::Delete($srcRoot, $true)

# ---- installer --------------------------------------------------------------
Step "installer"
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "C:\Program Files (x86)\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { Fail "Inno Setup is not installed (winget install JRSoftware.InnoSetup)" }
& $iscc "/DAppVersion=$version" "/DStageDir=$stage" "$root\installer\APFSReader.iss" | Select-Object -Last 4
if ($LASTEXITCODE -ne 0) { Fail "the installer did not build" }
$setup = Join-Path $dist "APFSReader-Setup-$version.exe"
if (-not (Test-Path $setup)) { Fail "no installer was produced" }

# ---- smoke test of the installer --------------------------------------------
if (-not $SkipSmokeTest) {
    Step "smoke test: silent per-user install, run, uninstall"
    $target = Join-Path $env:TEMP ("apfsreader-install-test-" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
    $log = Join-Path $env:TEMP "apfsreader-setup.log"
    $p = Start-Process $setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER", "/DIR=`"$target`"", "/TASKS=`"`"", "/LOG=`"$log`"" -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "the installer exited with $($p.ExitCode); see $log" }
    foreach ($f in $programs + @("LICENSE.md", "COPYING", "LICENSE-MIT", "THIRD_PARTY_LICENSES.md", "USER_GUIDE.ja.md", "USER_GUIDE.en.md", "unins000.exe")) {
        if (-not (Test-Path (Join-Path $target $f))) { Fail "the installer did not install $f" }
    }
    Write-Host "  ok  installed $((Get-ChildItem $target -File).Count) files"
    # The installed app must run and behave (minimise to tray and back).
    $st = Start-Process (Join-Path $target "apfsreader-gui.exe") -ArgumentList "--selftest" -PassThru
    if (-not $st.WaitForExit(90000)) { $st.Kill(); Fail "the installed app's self-test timed out" }
    if ($st.ExitCode -ne 0) { Fail "the installed app's self-test failed (exit $($st.ExitCode))" }
    Write-Host "  ok  installed app passes its self-test"
    $cli = & (Join-Path $target "apfsreader.exe") "disks" 2>&1 | Select-Object -First 1
    Write-Host "  ok  installed CLI runs: $cli"
    $u = Start-Process (Join-Path $target "unins000.exe") -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru
    Start-Sleep -Seconds 2
    if (Test-Path (Join-Path $target "apfsreader-gui.exe")) { Fail "the uninstaller left the program behind" }
    if (Test-Path $target) { Get-ChildItem $target -Force | ForEach-Object { Write-Host "  left behind: $($_.Name)" } }
    Write-Host "  ok  uninstalled"
}

# ---- checksums --------------------------------------------------------------
Step "checksums"
$sums = foreach ($f in $setup, $zip, $srcZip) {
    $h = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
    "$h *$(Split-Path $f -Leaf)"
}
[IO.File]::WriteAllText("$dist\SHA256SUMS.txt", (($sums -join "`n") + "`n"), (New-Object Text.UTF8Encoding($false)))
$sums | ForEach-Object { Write-Host "  $_" }

Write-Host "`nrelease $version is ready in $dist" -ForegroundColor Green
Get-ChildItem $dist -File | Select-Object Name, @{ n = "MB"; e = { [math]::Round($_.Length / 1MB, 2) } } | Format-Table -AutoSize | Out-String | Write-Host
