# APFSReader User Guide (version 0.9.0 beta)

APFSReader lets Windows open the disks you use on a Mac (**HFS+** and **APFS**) in Explorer,
**read-only**. It works with disk images (`.dmg` and similar) and with attached Mac disks.
Nothing is ever written to the disk.

> **This is beta software.** Reading physical disks has not yet been verified on real hardware.
> Keep a copy of anything important before you use it.

## Requirements

- Windows 10 / 11 (64-bit)
- **WinFsp**, a free file system driver. The app tells you if it is missing, and the installer can
  install it for you.

## Installing

- **Installer:** run `APFSReader-Setup-0.9.0.exe`. It normally installs without administrator rights.
- **ZIP:** extract to any folder and run `apfsreader-gui.exe`; keep `apfsreader-broker.exe` in the same folder.

The download is not code-signed, so Windows SmartScreen may warn on first run. Choose
**More info → Run anyway**. If an antivirus tool flags it, note that the program only reads disks and
presents them as virtual drives.

## Opening a disk image

1. Start `apfsreader-gui.exe`.
2. Press **Add image**, or drop a `.dmg` (or other image) onto the window.
3. The volumes inside are listed. Pick a drive letter (or leave it on *Auto*) and press **Mount**.
4. A new drive appears in Explorer; **Open folder** opens it too.
5. When you are done, press **Unmount**.

If an APFS disk has several volumes, the **Data** volume (where a Mac's user files live) is recommended.

## Opening an attached Mac disk

Click the **Physical disks** header (▼) to unfold it.

1. Press **Read this disk** for the disk you want.
2. A Windows administrator prompt (UAC) appears. Once approved, only a **small read-only helper** runs
   with administrator rights. The app and the drive keep your normal rights, so the drive shows up in
   Explorer as usual.
3. The disk is inspected and its volumes are listed; from here it works like an image.

The helper reads only the one disk you chose, has no way to write, and exits when the app closes.

## Tray and quitting

Closing or minimising the window keeps the app in the **system tray** and a notification tells you so.
Click the tray icon to bring the window back; the right-click menu has *Unmount all* and *Quit*.
Quitting unmounts everything.

## What works and what does not

| Works | Does not / note |
|---|---|
| HFS+ (including journaled), HFSX and APFS | **Encrypted APFS** (FileVault and similar) cannot be opened; it is listed with the reason |
| `.dmg` (zlib / bzip2 / LZFSE / LZMA / ADC), raw images, GPT and Apple Partition Map | Fusion Drives and APFS snapshots |
| Hard links, resource forks, extended attributes, transparently compressed files | Files using the newest compression (LZBITMAP) report an error when read |
| Japanese and other names (characters Windows forbids are shown as look-alikes) | Symbolic links appear as small files holding the target path |

An HFS+ disk that was not ejected cleanly on the Mac is read as it would be after its journal is replayed,
without writing anything.

## Command line (advanced)

```
apfsreader disks                         list attached disks
apfsreader info    image.dmg             summary of the contents
apfsreader ls      image.dmg /Documents  list a folder
apfsreader extract image.dmg /Documents D:\rescued
                                         copy a folder out; keeps going past read errors and reports them
apfsreader-mount   image.dmg R:          mount as a drive (Enter or Ctrl+C unmounts)
```

## Troubleshooting

- **"WinFsp is required":** install WinFsp from the download page, then press *Check again*.
- **You pressed No on the administrator prompt:** nothing is read; press *Read this disk* again.
- **"The disk could not be opened for reading":** another program may be using that disk.
- **Cannot open because it is encrypted:** encrypted volumes such as FileVault are not supported.

## Licences and credits

Copyright (c) 2026 niceokiraku

- The library (`core`), `apfsreader` and `apfsreader-broker` are **MIT**; the desktop app and the mount
  component are **GPL-3.0**. Full texts: `LICENSE-MIT` and `COPYING`; overview: `LICENSE.md`.
- Source code is published with every release as `APFSReader-<version>-source.zip`.
- Licences of the other software used are in `THIRD_PARTY_LICENSES.md`.

This software uses:

> **WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos**
> https://github.com/winfsp/winfsp

This software is provided without warranty.
