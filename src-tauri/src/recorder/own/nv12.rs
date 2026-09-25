//! BGRA to NV12 on the CPU, BT.709 limited range, with no Windows in it
//! (#239).
//!
//! The H.264 encoders take NV12, and the capture delivers BGRA. On a GPU the
//! conversion is the D3D11 video processor's (`own/win/scale.rs`), which is
//! where a recording does it. This is the fallback for a device that has no
//! video processor, which in practice is a machine encoding in software
//! **without** a GPU: the hosted CI runner (WARP only) and a VM. The software
//! H.264 MFT reads system memory anyway, so the frame has to come back to the
//! CPU either way, and converting it on the way costs one pass over it.
//!
//! The matrix and range are the ones the GPU path is told to use, and that
//! the encoder's media types declare: **BT.709, studio range** (Y 16-235,
//! chroma 16-240), which is what a 1080p H.264 file is assumed to be when it
//! says nothing, and what libobs writes. Chroma is the mean of each 2x2
//! block, sited at its centre; the H.264 default (left) siting differs by a
//! quarter of a chroma sample, which nothing in a review player can see.

/// Fixed-point scale for the coefficients: 2^16.
const SHIFT: u32 = 16;
const HALF: i32 = 1 << (SHIFT - 1);

/// BT.709 luma and chroma coefficients (Kr = 0.2126, Kb = 0.0722), scaled to
/// studio range (219/255 for Y, 224/255 for Cb and Cr) and by 2^16.
const Y_R: i32 = 11_966; // 0.2126 * 219/255 * 65536
const Y_G: i32 = 40_254; // 0.7152 * 219/255 * 65536
const Y_B: i32 = 4_064; // 0.0722 * 219/255 * 65536
const U_R: i32 = -6_596; // -0.2126/1.8556 * 224/255 * 65536
const U_G: i32 = -22_189; // -0.7152/1.8556 * 224/255 * 65536
const U_B: i32 = 28_784; // 0.5 * 224/255 * 65536
const V_R: i32 = 28_784; // 0.5 * 224/255 * 65536
const V_G: i32 = -26_145; // -0.7152/1.5748 * 224/255 * 65536
const V_B: i32 = -2_639; // -0.0722/1.5748 * 224/255 * 65536

/// The bytes an NV12 frame of `width` x `height` takes, tightly packed: a
/// full-size Y plane, then interleaved U and V at half size both ways.
pub fn frame_len(width: u32, height: u32) -> usize {
    let (w, h) = (width as usize, height as usize);
    w * h + w * (h / 2)
}

fn luma(r: i32, g: i32, b: i32) -> u8 {
    (16 + ((Y_R * r + Y_G * g + Y_B * b + HALF) >> SHIFT)).clamp(0, 255) as u8
}

fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = 128 + ((U_R * r + U_G * g + U_B * b + HALF) >> SHIFT);
    let v = 128 + ((V_R * r + V_G * g + V_B * b + HALF) >> SHIFT);
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

/// Converts a BGRA image to NV12, tightly packed (stride = `width`).
///
/// `bgra` is `height` rows of `pitch` bytes, each starting with `width`
/// pixels in B, G, R, A order (a mapped D3D11 texture's layout); alpha is
/// ignored. `width` and `height` must be even, which the encoded size always
/// is (`status::even_size`). `None` if they are not, or if `bgra` is too
/// short for them.
pub fn bgra_to_nv12(bgra: &[u8], pitch: usize, width: u32, height: u32) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 || pitch < w * 4 {
        return None;
    }
    if bgra.len() < pitch * (h - 1) + w * 4 {
        return None;
    }
    let mut out = vec![0u8; frame_len(width, height)];
    let (y_plane, uv_plane) = out.split_at_mut(w * h);
    let px = |x: usize, y: usize| {
        let i = y * pitch + x * 4;
        (i32::from(bgra[i + 2]), i32::from(bgra[i + 1]), i32::from(bgra[i]))
    };
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut sum = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (r, g, b) = px(x + dx, y + dy);
                y_plane[(y + dy) * w + x + dx] = luma(r, g, b);
                sum = (sum.0 + r, sum.1 + g, sum.2 + b);
            }
            // The mean of the four, rounded, then converted: the same as
            // converting each and averaging, since the transform is linear.
            let (u, v) = chroma((sum.0 + 2) / 4, (sum.1 + 2) / 4, (sum.2 + 2) / 4);
            let at = (y / 2) * w + x;
            uv_plane[at] = u;
            uv_plane[at + 1] = v;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w` x `h` image of one colour, with `pad` spare bytes a row, as a
    /// mapped texture with a wider pitch would be.
    fn solid(w: u32, h: u32, pad: usize, [b, g, r]: [u8; 3]) -> (Vec<u8>, usize) {
        let pitch = w as usize * 4 + pad;
        let mut img = vec![0xEE; pitch * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                img[y * pitch + x * 4..y * pitch + x * 4 + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        (img, pitch)
    }

    fn planes(nv12: &[u8], w: u32, h: u32) -> (&[u8], &[u8]) {
        nv12.split_at((w * h) as usize)
    }

    fn near(got: u8, want: u8) -> bool {
        got.abs_diff(want) <= 1
    }

    /// The reference values, from BT.709's own equations in studio range.
    #[test]
    fn primaries_and_greys_land_on_the_bt709_studio_values() {
        for (bgr, want) in [
            ([0, 0, 0], (16, 128, 128)),
            ([255, 255, 255], (235, 128, 128)),
            ([128, 128, 128], (126, 128, 128)),
            ([0, 0, 255], (63, 102, 240)),  // red
            ([0, 255, 0], (173, 42, 26)),   // green
            ([255, 0, 0], (32, 240, 118)),  // blue
        ] {
            let (img, pitch) = solid(4, 2, 0, bgr);
            let nv12 = bgra_to_nv12(&img, pitch, 4, 2).unwrap();
            let (y, uv) = planes(&nv12, 4, 2);
            assert!(y.iter().all(|&v| near(v, want.0)), "{bgr:?}: Y {y:?}, want {}", want.0);
            assert!(near(uv[0], want.1) && near(uv[1], want.2), "{bgr:?}: UV {uv:?}, want {want:?}");
            assert_eq!(uv[0], uv[2]);
            assert_eq!(uv[1], uv[3]);
        }
    }

    #[test]
    fn the_layout_is_a_y_plane_then_interleaved_half_size_chroma() {
        assert_eq!(frame_len(320, 240), 320 * 240 * 3 / 2);
        assert_eq!(frame_len(1920, 1080), 1920 * 1080 * 3 / 2);
        let (img, pitch) = solid(320, 240, 0, [10, 20, 30]);
        assert_eq!(bgra_to_nv12(&img, pitch, 320, 240).unwrap().len(), frame_len(320, 240));
    }

    /// A mapped texture's rows are wider than its pixels; the padding is
    /// never read as a pixel.
    #[test]
    fn the_row_pitch_is_honoured() {
        let (img, pitch) = solid(6, 4, 40, [0, 0, 0]);
        let nv12 = bgra_to_nv12(&img, pitch, 6, 4).unwrap();
        let (y, uv) = planes(&nv12, 6, 4);
        assert!(y.iter().all(|&v| v == 16), "{y:?}");
        assert!(uv.iter().all(|&v| v == 128), "{uv:?}");
    }

    /// Each 2x2 block's chroma is its own mean: a left half black and a
    /// right half white give grey chroma per block and a hard edge in luma.
    #[test]
    fn chroma_is_per_2x2_block_and_luma_is_per_pixel() {
        let (w, h) = (4u32, 2u32);
        let pitch = w as usize * 4;
        let mut img = vec![0u8; pitch * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let v = if x < 2 { 0 } else { 255 };
                img[y * pitch + x * 4..y * pitch + x * 4 + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let nv12 = bgra_to_nv12(&img, pitch, w, h).unwrap();
        let (y, uv) = planes(&nv12, w, h);
        assert_eq!(y, &[16, 16, 235, 235, 16, 16, 235, 235]);
        assert_eq!(uv, &[128, 128, 128, 128]);
    }

    #[test]
    fn odd_sizes_and_short_buffers_are_refused() {
        let (img, pitch) = solid(4, 4, 0, [0, 0, 0]);
        assert!(bgra_to_nv12(&img, pitch, 3, 4).is_none());
        assert!(bgra_to_nv12(&img, pitch, 4, 3).is_none());
        assert!(bgra_to_nv12(&img, pitch, 0, 4).is_none());
        assert!(bgra_to_nv12(&img[..img.len() - 1], pitch, 4, 4).is_none());
        assert!(bgra_to_nv12(&img, 8, 4, 4).is_none(), "a pitch narrower than a row");
    }
}
