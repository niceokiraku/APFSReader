//! Writes the app icon as a .ico file (for the installer):
//! `cargo run -p apfsreader-icon --example make_ico -- out.ico`
fn main() {
    let path = std::env::args().nth(1).expect("usage: make_ico <out.ico>");
    std::fs::write(path, apfsreader_icon::ico()).expect("write ico");
}
