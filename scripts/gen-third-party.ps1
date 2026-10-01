# Regenerates THIRD_PARTY_LICENSES.md: the licence texts of every crate that is
# linked into the shipped programs, plus the hand-written notices in
# scripts\third-party-header.md.
#   powershell -File scripts\gen-third-party.ps1
#
# Only normal (run-time) dependencies count: build tools and test libraries are
# not in the binaries. Identical licence texts are printed once, followed by the
# crates they cover.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$tree = cargo tree --workspace -e normal --target x86_64-pc-windows-msvc --prefix none --format "{p}|{l}|{r}" 2>$null
$crates = @{}
foreach ($line in $tree) {
    if ($line -match "proc-macro") { continue }
    $line = $line -replace "\s*\(\*\)\s*$", ""
    $parts = $line -split "\|"
    if ($parts.Count -lt 2) { continue }
    if ($parts[0] -notmatch "^(?<name>\S+) v(?<ver>\S+)") { continue }
    if ($Matches.name -like "apfsreader*") { continue }
    $crates["$($Matches.name) $($Matches.ver)"] = @{
        name = $Matches.name; version = $Matches.ver; license = $parts[1]; repo = $(if ($parts.Count -gt 2) { $parts[2] } else { "" })
    }
}

$registry = Get-ChildItem "$env:USERPROFILE\.cargo\registry\src" -Directory | ForEach-Object { $_.FullName }
function Find-Crate($name, $version) {
    foreach ($r in $registry) {
        $d = Join-Path $r "$name-$version"
        if (Test-Path $d) { return $d }
    }
    return $null
}

# Licence-like files at the crate root, and in the font folder of egui's bundled fonts.
$pattern = "^(LICEN[SC]E|COPYING|UNLICENSE|NOTICE|OFL|UFL)([-_.].*)?$|license"
$byText = @{}
$missing = @()
foreach ($key in ($crates.Keys | Sort-Object)) {
    $c = $crates[$key]
    $dir = Find-Crate $c.name $c.version
    if (-not $dir) { $missing += $key; continue }
    $files = @(Get-ChildItem $dir -File | Where-Object { $_.Name -match $pattern -and $_.Name -notmatch "\.(rs|toml|ttf)$" })
    if ($c.name -eq "epaint_default_fonts") {
        $files += Get-ChildItem (Join-Path $dir "fonts") -File -Filter "*.txt"
    }
    if ($files.Count -eq 0) { $missing += "$key (no licence file in the package; declared: $($c.license))"; continue }
    foreach ($f in $files) {
        $text = (Get-Content $f.FullName -Raw -Encoding UTF8).Trim()
        $h = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($text)))
        if (-not $byText.ContainsKey($h)) { $byText[$h] = @{ text = $text; users = @(); file = $f.Name } }
        $byText[$h].users += "$($c.name) $($c.version)"
    }
}

$sb = New-Object Text.StringBuilder
[void]$sb.AppendLine((Get-Content "$PSScriptRoot\third-party-header.md" -Raw -Encoding UTF8).TrimEnd())
[void]$sb.AppendLine("")
[void]$sb.AppendLine("## Rust crates linked into the programs")
[void]$sb.AppendLine("")
[void]$sb.AppendLine("The following $($crates.Count) crates are linked into one or more of the programs. Each is used under the")
[void]$sb.AppendLine("licence it declares; the declared licence is listed, then the licence texts that ship with the crates.")
[void]$sb.AppendLine("")
[void]$sb.AppendLine("| Crate | Version | Declared licence |")
[void]$sb.AppendLine("|---|---|---|")
foreach ($key in ($crates.Keys | Sort-Object)) {
    $c = $crates[$key]
    [void]$sb.AppendLine("| $($c.name) | $($c.version) | $($c.license) |")
}
[void]$sb.AppendLine("")
[void]$sb.AppendLine("## Licence texts")
$n = 0
foreach ($h in ($byText.Keys | Sort-Object { $byText[$_].users.Count } -Descending)) {
    $n++
    $e = $byText[$h]
    $users = ($e.users | Sort-Object -Unique) -join ", "
    [void]$sb.AppendLine("")
    [void]$sb.AppendLine("### Text $n ($($e.file)) - used by: $users")
    [void]$sb.AppendLine("")
    [void]$sb.AppendLine('```text')
    [void]$sb.AppendLine($e.text)
    [void]$sb.AppendLine('```')
}
if ($missing.Count -gt 0) {
    [void]$sb.AppendLine("")
    [void]$sb.AppendLine("## Crates whose package carries no separate licence file")
    [void]$sb.AppendLine("")
    [void]$sb.AppendLine("Their declared licence (see the table above) applies. Where that is MIT OR Apache-2.0 the Apache-2.0")
    [void]$sb.AppendLine("option is used, and its standard text is among the texts above. The two WinFsp bindings are under")
    [void]$sb.AppendLine("GPL-3.0, whose text is the COPYING file of this distribution.")
    foreach ($m in $missing) { [void]$sb.AppendLine("- $m") }
}
[IO.File]::WriteAllText("$root\THIRD_PARTY_LICENSES.md", $sb.ToString(), (New-Object Text.UTF8Encoding($false)))
Write-Host "crates: $($crates.Count); distinct licence texts: $($byText.Count); without a licence file: $($missing.Count)"
$missing | ForEach-Object { Write-Host "  no file: $_" }
