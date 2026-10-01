# Third-party licences

APFSReader's own licences are described in `LICENSE.md` (MIT for the library and the command
line tool, GPL-3.0 for the mount component and the desktop app). This file lists what else is
included in, or required by, the programs.

## WinFsp

> WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos
> https://github.com/winfsp/winfsp

The mount component and the desktop app are built on the WinFsp file system framework, which
makes the volumes appear as drives. They link against WinFsp's DLL interface (through the
`winfsp-rs` bindings, GPL-3.0) under the GPLv3 and the FLOSS exception in WinFsp's licence.
The WinFsp driver itself is **not** part of APFSReader: it is installed separately, as the
unmodified installer from the WinFsp project, and is covered by WinFsp's own licence
(GPLv3 with the FLOSS exception; commercial licences are also available from its author).

## Fonts

The desktop app draws its text with **Segoe UI**, **Yu Gothic** and **Cascadia Mono** from the
user's own Windows installation; those fonts are not distributed with APFSReader. egui's
built-in fallback fonts (Hack, Ubuntu-Light, Noto Emoji, emoji-icon-font) are compiled into the
desktop app; their licences (MIT, Ubuntu Font Licence 1.0, SIL OFL 1.1) are reproduced below
under `epaint_default_fonts`.

## LZVN decoder (core/src/lzvn.rs)

A Rust port of the LZVN decoder from Apple's reference LZFSE implementation
(https://github.com/lzfse/lzfse, `lzvn_decode_base.c`), taken via the Go port in
go-apfs-v2 (MIT / BSD-3-Clause).

```
Copyright (c) 2015-2016, Apple Inc. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

1.  Redistributions of source code must retain the above copyright notice,
    this list of conditions and the following disclaimer.

2.  Redistributions in binary form must reproduce the above copyright notice,
    this list of conditions and the following disclaimer in the documentation
    and/or other materials provided with the distribution.

3.  Neither the name of the copyright holder(s) nor the names of any contributors
    may be used to endorse or promote products derived from this software without
    specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT
SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR
BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN
ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH
DAMAGE.
```
