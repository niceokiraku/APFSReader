# Test data

The tests that need disk images look for them here and **skip themselves when they are absent**,
so `cargo test` works on a fresh clone. The images are not part of this repository: they belong to
other projects, and some are large. To run the full suite, put them in these folders.

| Folder / file | What it is | Where it comes from |
|---|---|---|
| `macos-fixtures/` | Images written by macOS (`hdiutil`, `ditto`, `xattr`) in APFS and HFS+, in raw, GPT, Apple Partition Map and every DMG codec, with `manifest.json` / `hfs-manifest.json` recording the expected SHA-256 of each file. The `.img.gz` files are gunzipped to `.img`. | `testdata/cli/` of [go-apfs-v2](https://github.com/deploymenttheory/go-apfs-v2) (MIT); `scripts/gen-fixtures.sh` there shows how they were made |
| `nps-hfsjtest1/` | `image.gen0.dmg`, `image.gen1.dmg`: journaled HFS+ volumes (raw images despite the name) | NPS Test Disk Images, `nps-2009-hfsjtest1`, from the [Digital Corpora](https://digitalcorpora.org/corpora/disk-images/) drives collection |
| `UNENCRYPTED.dmg` | An APFS image made by macOS Disk Utility in 2017 (raw disk with a GPT) | Linked from <https://thinkdfir.com/2017/09/27/playing-with-apfs/> |
| `gen/`, `gen-tree/` | Synthetic images and the tree they are built from | `scripts/gen-testdata.ps1`, which needs a built [go-apfs-v2](https://github.com/deploymenttheory/go-apfs-v2) `apfs.exe` in `tools/go-apfs-v2/` |

The mount and GUI checks (`scripts/test-mount.ps1`, `scripts/test-gui.ps1`) use the same images.
