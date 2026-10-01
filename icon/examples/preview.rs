//! Writes a PNG sheet of the icon at several sizes on light and dark backgrounds:
//! `cargo run -p apfsreader-icon --example preview -- out.png`
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "icon-preview.png".into());
    let sizes = [256u32, 128, 64, 48, 32, 24, 16];
    let (w, h) = (256 + 128 + 64 + 48 + 32 + 24 + 16 + 8 * 20, 300u32);
    let mut sheet = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let dark = y >= h / 2 + 20 || false;
            let bg: [u8; 3] = if x < w / 2 { [245, 247, 250] } else { [20, 24, 32] };
            let _ = dark;
            let i = ((y * w + x) * 4) as usize;
            sheet[i..i + 3].copy_from_slice(&bg);
            sheet[i + 3] = 255;
        }
    }
    let mut x0 = 10u32;
    for &s in &sizes {
        let px = apfsreader_icon::rgba(s);
        let y0 = (h - s) / 2;
        for y in 0..s {
            for x in 0..s {
                let a = px[((y * s + x) * 4 + 3) as usize] as u32;
                let i = (((y0 + y) * w + x0 + x) * 4) as usize;
                for c in 0..3 {
                    let v = px[((y * s + x) * 4) as usize + c] as u32;
                    sheet[i + c] = ((v * a + sheet[i + c] as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
        x0 += s + 20;
    }
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&sheet).unwrap();
    }
    std::fs::write(path, out).unwrap();
}
