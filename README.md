# APFSReader

Read-only access to macOS volumes from Windows: **HFS+ / HFSX** and **APFS**
(unencrypted), from disk images and physical disks. A small desktop app mounts
them as ordinary drives in Explorer; minimising it keeps it in the system tray.

| Crate / program | Purpose | Licence |
|---|---|---|
| `core` | Parsers and drivers (HFS+ with journal replay, APFS, UDIF `.dmg`, GPT, Apple Partition Map, decmpfs), physical-disk access, the helper protocol | MIT |
| `cli` → `apfsreader` | `parts`, `info`, `ls`, `cat`, `extract`, `disks` | MIT |
| `broker` → `apfsreader-broker` | The only part that runs as administrator: serves raw reads of one physical disk | MIT |
| `mount` → `apfsreader-mount` | Mounts a volume as a Windows drive via WinFsp (command line, and the library the app uses) | GPL-3.0 (winfsp-rs) |
| `gui` → `apfsreader-gui` | The desktop app | GPL-3.0 (links the above) |
| `icon`, `build-resources` | The app icon, drawn in code, and the build step that embeds it in every `.exe` (same picture in Explorer, taskbar, tray and window) | MIT |

Copyright (c) 2026 niceokiraku. The licences are `LICENSE-MIT` (MIT parts) and `COPYING`
(GPL-3.0 parts); `LICENSE.md` explains which is which and `THIRD_PARTY_LICENSES.md` reproduces the
licences of everything compiled in. Users: see `docs/USER_GUIDE.ja.md` (Japanese) or
`docs/USER_GUIDE.en.md`. **Beta (0.9.0):** reading physical disks is not yet verified on
real hardware.

## Credits

APFSReader uses WinFsp to show volumes as drives:

> **WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos**
> https://github.com/winfsp/winfsp

## The app

Run `apfsreader-gui.exe` (WinFsp must be installed). The window lists

- **Disk images** you add with *Add image…* or by dropping a `.dmg` / raw image on
  the window. Each is inspected at once and its volumes are listed.
- **Physical disks** attached to the PC, in a section that is **folded by default**
  (click its header, marked with a chevron, to unfold it). Nothing is read until you
  press *Read this disk*, which asks for administrator approval (see below).

For each volume pick a drive letter (or *Auto*) and press *Mount*. Mounted volumes
are shown at the top with *Open in Explorer* and *Unmount*. An APFS container with
several volumes recommends its **Data** volume (where a Mac's user files are).
Encrypted APFS volumes are listed but cannot be mounted.

The window uses a light theme (Segoe UI and Yu Gothic fonts). Closing the window or
minimising it moves it to the **system tray** and a balloon notification from the tray
icon says so; click the tray icon to bring it back, or use its menu (*Show window*, *Unmount all*, *Quit*).
Quitting unmounts everything. A second launch just brings the running copy forward.
The interface is Japanese on a Japanese Windows and English otherwise
(`APFSREADER_LANG=en` or `ja` overrides). Command line: image paths to add,
`--mount` to mount them straight away, `--minimized` to start in the tray.

### Physical disks and administrator rights

Reading a raw disk needs administrator rights, but a drive letter created by an
elevated program is not shown in the normal Explorer. So the app and the mount run
as the normal user, and only a tiny helper, `apfsreader-broker.exe`, is elevated
(you see one UAC prompt). The helper

- opens exactly one **physical disk, read-only** (it never accepts a file path, so
  it cannot be used to read protected files), and has no write operation at all;
- serves reads over a named pipe that only your account can open, rejects remote
  clients, and requires a random token given at launch;
- exits as soon as the app disconnects, or if nothing connects within 90 seconds.

If you start the app already elevated, the disk is read directly with no helper.
Listing disks needs no rights, so the list appears at once.

## Command line

```
apfsreader disks                                          # list physical disks
apfsreader info    image.dmg
apfsreader ls      image.dmg /Documents
apfsreader extract image.dmg /Documents D:\rescued        # copy a folder out; -v lists files
apfsreader-mount   image.dmg R:                           # Enter or Ctrl+C unmounts
```

`image` may be a raw image, a UDIF `.dmg`, a whole-disk image with a GPT or Apple
Partition Map, or `\\.\PhysicalDriveN` (run as administrator; for a drive that
Explorer can see, use the app instead). `--part N` picks a partition, `--vol N` an
APFS volume (default: the Data volume if there is one).

`extract` is meant for rescue work: it carries on past read errors, keeps the
readable part of damaged files, prints every problem at the end and exits with
status 3 if there were any. Names Windows cannot hold (`:`, `?`, reserved names
such as `CON`) are replaced by look-alikes; symbolic links are written as
`name.symlink` text files. `APFSREADER_TRACE=1` makes `apfsreader-mount` log every request.

## What is supported

- **File systems:** HFS+, HFSX, APFS (current state; snapshots are not exposed).
- **Containers:** raw, UDIF (zlib, bzip2, LZFSE, XZ-framed LZMA, ADC), GPT (512-byte
  and 4Kn sectors), Apple Partition Map.
- **Files:** hard links, symbolic links, extended attributes, resource forks,
  transparent compression (decmpfs types 1, 3/4, 7/8, 11/12).
- **HFS+ journal:** a volume that was not cleanly unmounted is read as it would be
  after a journal replay, done in memory; nothing is written.
- **Never written to.** The mount is read-only and no component has a write path.

## What is not

- Encrypted APFS volumes (refused, not misread) and Fusion Drive containers.
- decmpfs types 9/10, 13/14 (LZBITMAP) and 5: such files report an error.
- APFS snapshots, sealed-volume details and merging a System/Data volume pair.
- HFS+ journals on another device.
- Symbolic links as Windows links: they appear as small files holding the target.
- Automatic recognition of a disk the moment it is plugged in: the list refreshes
  every few seconds while the window is open, or press *Refresh disks*.

## How well is it verified?

`cargo test` (129 tests), `scripts\test-mount.ps1` (mounts fixtures and checks
them through Windows file APIs) and `scripts\test-gui.ps1` (drives the real window)
all pass; build with `cargo build --release` first for the scripts.

- **Real macOS output.** `testdata/macos-fixtures` holds images made with `hdiutil`,
  `ditto` and `xattr`, with SHA-256 manifests recorded at creation (they come from
  go-apfs-v2's repository; see `scripts/gen-fixtures.sh` there). Every entry is
  checked in APFS and HFS+, in raw, GPT, APM and each DMG codec above.
  `testdata/UNENCRYPTED.dmg` is a 2017 Disk Utility image and
  `testdata/nps-hfsjtest1` are journaled HFS+ volumes from the NPS corpus, whose
  logs confirmed the journal layout and the 512-byte block unit.
- **Only partly covered by real output.** Of the decmpfs types only LZVN in a
  resource fork (type 8) occurs in the fixtures; zlib and LZFSE use synthetic
  vectors. Journal replay is exercised by rewinding the real journals as an unclean
  shutdown would and damaging the blocks they rewrite, plus a hand-built log that
  wraps round its end in both byte orders. No test image comes from a volume that
  was genuinely left dirty by a crash.
- **Physical disks: everything except the disk itself.** Listing real disks, the
  sector-aligned reader (against a fake device that refuses misaligned requests),
  4Kn partition tables, the helper protocol over a real named pipe (wrong tokens,
  oversized reads, timeouts) and whole volumes read through that pipe are tested.
  **Not tested: reading a real physical disk, the UAC prompt and the elevated
  helper.** Those need a real disk and an administrator session, so the first run
  on hardware is the real test.
- **The window.** Layout, state handling and every button's logic (against a fake
  backend); minimise-to-tray, restore from a hidden window, close-to-tray, mounting
  and unmounting through the real window. **Not tested: clicking the tray icon or
  its menu by hand, the Explorer window itself, Windows' own notification-area
  overflow, or the file dialog.**
- **Robustness.** Damaged copies of every fixture (random bit flips, tens of
  thousands of them in a long run) are walked completely; the result must be an
  error or valid data, never a panic. `FUZZ_ROUNDS=20000 cargo test --test
  fs_robustness` runs a long soak.

Images from go-apfs-v2 (`scripts/gen-testdata.ps1`) cross-check parsing but are a
third-party writer, not Apple's.

## Build

Rust (MSVC toolchain). The mount crate needs libclang for bindgen; the project looks
for it in `tools/libclang` (`python -m pip install --target tools/libclang
libclang`). WinFsp must be installed to mount.

```
cargo build --release
cargo test
```

`apfsreader-gui.exe` expects `apfsreader-broker.exe` in the same folder. The test
images in `testdata/` are not part of the source distribution (they come from other
projects); tests that need them skip themselves when they are absent, and
`scripts/gen-testdata.ps1` regenerates the synthetic ones.

## Releasing

```
powershell -File scripts\release.ps1        # tests, build, licences, installer, ZIPs, checksums
```

The script writes `dist/APFSReader-Setup-<version>.exe` (Inno Setup; installs per user without
administrator rights and offers to install WinFsp through winget), the portable
`APFSReader-<version>-win64.zip`, `APFSReader-<version>-source.zip` (the GPL parts require the
source to be published with the binaries) and `SHA256SUMS.txt`. Before it finishes it checks that
no build-machine path or user name is inside any program, that every program has the icon and the
version, and that the installer installs, runs and uninstalls cleanly. The version comes from
`Cargo.toml`. Builds are unsigned, so Windows SmartScreen may warn on first run.

`scripts/gen-third-party.ps1` regenerates `THIRD_PARTY_LICENSES.md`; run it after changing dependencies.
