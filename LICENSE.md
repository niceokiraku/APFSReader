# Licences

Copyright (c) 2026 niceokiraku

APFSReader is made of several parts under two licences:

| Part | Licence | Text |
|---|---|---|
| `core` (the library), `cli` (`apfsreader`), `broker` (`apfsreader-broker`), `icon`, `build-resources` | MIT | `LICENSE-MIT` |
| `mount` (`apfsreader-mount`) and `gui` (`apfsreader-gui`) | GPL-3.0-only | `COPYING` |

The mount component and the desktop app link `winfsp-rs`, which is GPL-3.0, so they are GPL-3.0
too. The libraries they build on stay MIT, so they can be reused on their own.

The installer and the ZIP contain the programs in binary form. As the GPL requires, the
corresponding source code (all parts, including the build scripts) is published together with
each release as `APFSReader-<version>-source.zip`.

## WinFsp

> WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos
> https://github.com/winfsp/winfsp

APFSReader needs WinFsp to show volumes as drives. WinFsp is installed separately; its licence
(GPLv3 with a FLOSS exception) is in `C:\Program Files (x86)\WinFsp\License.txt` once installed.

## Everything else

`THIRD_PARTY_LICENSES.md` lists the other software that is compiled into the programs and
reproduces its licences.

## No warranty

The programs are provided "as is", without warranty of any kind. They only read from disks and
disk images, but this is beta software: do not rely on it as the only way to reach data you
cannot lose, and keep a copy of anything important.
