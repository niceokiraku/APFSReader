//! Damaged DMGs must produce errors, never panics, in the container parser
//! and in every codec (including the third-party bzip2, XZ and LZFSE ones).
//! Skipped when the fixtures are absent.

use apfsreader_core::device::BlockSource;
use apfsreader_core::udif::UdifSource;
use std::path::PathBuf;

const IMAGES: [&str; 5] =
    ["basic.dmg", "basic-bz2.dmg", "basic-lzfse.dmg", "hfs-basic-adc.dmg", "hfs-basic-lzma.dmg"];

#[test]
fn corrupted_dmgs_never_panic() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/macos-fixtures");
    let tmp = std::env::temp_dir().join(format!("apfsreader-dmg-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for name in IMAGES {
        let Ok(original) = std::fs::read(dir.join(name)) else { continue };
        let path = tmp.join(name);
        for round in 0..40 {
            let mut data = original.clone();
            for _ in 0..(1 + next() % 4) {
                // Bias toward the start (chunk data) and the end (trailer + table).
                let pos = if round % 4 == 0 {
                    data.len() - 1 - (next() as usize) % 4096.min(data.len())
                } else {
                    (next() as usize) % data.len()
                };
                data[pos] ^= 1 << (next() % 8);
            }
            std::fs::write(&path, &data).unwrap();
            if let Ok(src) = UdifSource::open(&path) {
                // Touch the start, the middle and the end of the logical disk.
                let len = src.len();
                for off in [0, len / 2, len.saturating_sub(65536)] {
                    let mut buf = vec![0u8; 65536.min((len - off) as usize)];
                    let _ = src.read_at(off, &mut buf);
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
