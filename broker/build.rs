//! Embeds the APFSReader icon and version information in the executable, so
//! Explorer shows the same picture as the window and the tray.
fn main() {
    apfsreader_build_resources::embed("APFSReader disk helper (read-only)");
}
