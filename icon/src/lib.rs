//! The APFSReader icon, drawn in code so one picture serves the window, the
//! tray and the `.exe` file icon Explorer shows.
//!
//! The design: a blue-to-teal rounded tile holding a white disc (a drive
//! platter) with a small padlock badge, for "read-only". The padlock is left out
//! at very small sizes where it would only be noise.

const BLUE: [f32; 3] = [52.0, 92.0, 255.0];
const TEAL: [f32; 3] = [18.0, 196.0, 184.0];
const NAVY: [f32; 3] = [16.0, 26.0, 56.0];

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Signed distance to a rounded rectangle centred at (cx, cy).
fn round_rect(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (x - cx).abs() - (hw - r);
    let qy = (y - cy).abs() - (hh - r);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

fn dist(x: f32, y: f32, cx: f32, cy: f32) -> f32 {
    ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
}

/// Colour of the icon at one point of the unit square, as straight RGBA in 0..1.
fn sample(x: f32, y: f32, with_lock: bool) -> [f32; 4] {
    // Tile.
    if round_rect(x, y, 0.5, 0.5, 0.5, 0.5, 0.22) > 0.0 {
        return [0.0; 4];
    }
    let mut c = mix(BLUE, TEAL, ((x + y) / 2.0).clamp(0.0, 1.0));
    // A soft highlight along the top edge.
    let gloss = (1.0 - y * 2.2).clamp(0.0, 1.0) * 0.14;
    c = mix(c, [255.0, 255.0, 255.0], gloss);

    // Disc.
    let (dx, dy, r) = (0.455, 0.455, 0.30);
    let d = dist(x, y, dx, dy);
    if d < r {
        let shade = ((y - (dy - r)) / (2.0 * r)).clamp(0.0, 1.0);
        c = mix([250.0, 252.0, 255.0], [214.0, 226.0, 248.0], shade);
        if d > r - 0.022 {
            c = mix(c, BLUE, 0.35); // rim
        }
        if (d - 0.205).abs() < 0.006 {
            c = mix(c, BLUE, 0.28); // groove
        }
        if (d - 0.135).abs() < 0.005 {
            c = mix(c, BLUE, 0.22);
        }
        if d < 0.068 {
            // The hub is a hole through to the tile.
            c = mix(BLUE, TEAL, ((x + y) / 2.0).clamp(0.0, 1.0));
        }
    }

    // Read-only badge.
    if with_lock {
        let (bx, by, br) = (0.74, 0.74, 0.185);
        let bd = dist(x, y, bx, by);
        if bd < br + 0.02 {
            c = if bd < br { NAVY } else { [255.0, 255.0, 255.0] };
        }
        if bd < br {
            let white = [255.0, 255.0, 255.0];
            // Body.
            let body_cy = by + 0.034;
            let body = round_rect(x, y, bx, body_cy, 0.07, 0.05, 0.012);
            // Shackle: a half ring on top of two straight legs reaching the body.
            let ring_cy = by - 0.024;
            let sd = dist(x, y, bx, ring_cy);
            let (outer, inner) = (0.05, 0.028);
            let arc = sd < outer && sd > inner && y <= ring_cy;
            let legs = (x - bx).abs() > inner
                && (x - bx).abs() < outer
                && y > ring_cy
                && y < body_cy - 0.03;
            if body < 0.0 || arc || legs {
                c = white;
            }
            // Keyhole.
            if dist(x, y, bx, body_cy - 0.004) < 0.014 {
                c = NAVY;
            }
        }
    }
    [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, 1.0]
}

/// RGBA pixels (straight alpha) of a `size`×`size` icon.
pub fn rgba(size: u32) -> Vec<u8> {
    let with_lock = size >= 28;
    let n = if size <= 64 { 4 } else { 3 }; // n×n samples per pixel
    let mut out = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..n {
                for sx in 0..n {
                    let x = (px as f32 + (sx as f32 + 0.5) / n as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / n as f32) / size as f32;
                    let s = sample(x, y, with_lock);
                    // Accumulate premultiplied so the edge colour is not dragged toward black.
                    r += s[0] * s[3];
                    g += s[1] * s[3];
                    b += s[2] * s[3];
                    a += s[3];
                }
            }
            let i = ((py * size + px) * 4) as usize;
            if a > 0.0 {
                out[i] = (r / a * 255.0).round() as u8;
                out[i + 1] = (g / a * 255.0).round() as u8;
                out[i + 2] = (b / a * 255.0).round() as u8;
                out[i + 3] = (a / (n * n) as f32 * 255.0).round() as u8;
            }
        }
    }
    out
}

/// The icon as a PNG file.
pub fn png(size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("PNG header");
        w.write_image_data(&rgba(size)).expect("PNG data");
    }
    out
}

/// A Windows `.ico` holding the icon at every size Explorer asks for.
pub fn ico() -> Vec<u8> {
    const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&s| png(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (s, img) in SIZES.iter().zip(&images) {
        out.push(if *s >= 256 { 0 } else { *s as u8 }); // width (0 means 256)
        out.push(if *s >= 256 { 0 } else { *s as u8 });
        out.push(0); // palette
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(px: &[u8], size: u32, x: u32, y: u32) -> u8 {
        px[((y * size + x) * 4 + 3) as usize]
    }

    #[test]
    fn the_corner_is_transparent_and_the_middle_opaque() {
        for size in [16u32, 32, 64, 256] {
            let px = rgba(size);
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert_eq!(alpha(&px, size, 0, 0), 0, "size {size}");
            assert_eq!(alpha(&px, size, size / 2, size / 2), 255, "size {size}");
        }
    }

    #[test]
    fn the_disc_is_light_on_the_tile() {
        let px = rgba(128);
        let i = ((40 * 128 + 40) * 4) as usize; // inside the disc, upper left
        assert!(px[i] > 200 && px[i + 1] > 200 && px[i + 2] > 200, "{:?}", &px[i..i + 4]);
        let corner = ((10 * 128 + 64) * 4) as usize; // on the tile, above the disc
        assert!(px[corner + 2] > px[corner], "the tile is blue-ish");
    }

    #[test]
    fn the_png_decodes_to_the_same_pixels() {
        let data = png(48);
        let mut reader = png::Decoder::new(std::io::Cursor::new(data)).read_info().unwrap();
        let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (48, 48));
        assert_eq!(&buf[..info.buffer_size()], &rgba(48)[..]);
    }

    #[test]
    fn the_ico_is_well_formed() {
        let ico = ico();
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        assert_eq!(count, 7);
        for i in 0..count {
            let e = 6 + 16 * i;
            let len = u32::from_le_bytes(ico[e + 8..e + 12].try_into().unwrap()) as usize;
            let off = u32::from_le_bytes(ico[e + 12..e + 16].try_into().unwrap()) as usize;
            assert_eq!(&ico[off..off + 8], b"\x89PNG\r\n\x1a\n", "entry {i} is a PNG");
            assert!(off + len <= ico.len());
        }
    }
}
