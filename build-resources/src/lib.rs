//! For use from `build.rs`: gives the executable the APFSReader icon and
//! version information, so Explorer, the taskbar, the tray and the window all
//! show the same picture.

use std::path::PathBuf;

pub fn embed(description: &str) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("apfsreader.ico");
    std::fs::write(&ico, apfsreader_icon::ico()).expect("write icon");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().expect("icon path"));
    res.set("ProductName", "APFSReader");
    res.set("FileDescription", description);
    res.set("LegalCopyright", "APFSReader");
    res.compile().expect("compile Windows resources (needs rc.exe from the Windows SDK)");
}
