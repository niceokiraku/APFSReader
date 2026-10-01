# Regenerates testdata/gen-tree and the HFS+/APFS DMGs under testdata/gen using
# go-apfs-v2 (tools/go-apfs-v2, built with: go build -o apfs.exe ./cmd/apfs).
# Output is deterministic (fixed epoch, seeded random file).
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$t = "testdata\gen-tree"
New-Item -ItemType Directory -Force "$t\docs\deep\er\est", "$t\日本語フォルダ", "$t\many" | Out-Null
[IO.File]::WriteAllText("$root\$t\hello.txt", "Hello, APFSReader`n")
[IO.File]::WriteAllText("$root\$t\日本語フォルダ\テスト.txt", "日本語の内容です`n")
[IO.File]::WriteAllText("$root\$t\docs\deep\er\est\leaf.txt", "deep leaf`n")
1..300 | ForEach-Object { [IO.File]::WriteAllText("$root\$t\many\file_$_.txt", "content $_`n") }
$rng = New-Object Random 42
$buf = New-Object byte[] (5MB)
$rng.NextBytes($buf)
[IO.File]::WriteAllBytes("$root\$t\random5m.bin", $buf)

New-Item -ItemType Directory -Force testdata\gen | Out-Null
foreach ($fs in "hfs+", "apfs") {
    $n = $fs.Replace("+", "plus")
    & tools\go-apfs-v2\apfs.exe pack $t "testdata\gen\$n-none.dmg" --fs $fs --volname TestVol `
        --compression none --source-date-epoch 1700000000
}
