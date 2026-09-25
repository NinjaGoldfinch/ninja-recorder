//! Where a captured frame goes in the fixed-size output frame.
//!
//! The own backend picks its output size once, at `start`, from the game
//! window's size then, and the encoder is set up for exactly that. The window
//! can change size afterwards: a resolution change, a switch between
//! windowed and borderless, a drag of a windowed border. WGC follows it and
//! delivers frames at the new size, and something has to decide how those
//! land in a frame that did not change.
//!
//! **Scaled, aspect preserved, centred, bars black** — what libobs's window
//! capture does in a canvas of fixed size, so the two backends' files look the
//! same after a resize. The spike cropped a window that grew and left stale
//! edges around one that shrank (DEVELOPMENT.md §2.2, "a resize scales; it
//! does not crop").
//!
//! Pure, so the maths is tested on every platform; `win::scale` does what it
//! says with the D3D11 video processor.

/// A width and a height in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Size {
        Size { width, height }
    }

    /// A WGC `SizeInt32`'s two fields; a negative one is no size at all.
    pub fn from_i32(width: i32, height: i32) -> Size {
        Size { width: width.max(0) as u32, height: height.max(0) as u32 }
    }

    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// A rectangle inside the output frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn right(self) -> u32 {
        self.left + self.width
    }

    pub fn bottom(self) -> u32 {
        self.top + self.height
    }

    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Rounds down to even. H.264 in 4:2:0 wants even dimensions, and the NV12
/// surface #239 converts into has a chroma sample per 2x2 block, so an odd
/// edge or an odd offset would split one.
const fn even(v: u32) -> u32 {
    v & !1
}

/// `a * b / c`, rounded to nearest, in 64 bits so no size overflows.
fn scale(a: u32, b: u32, c: u32) -> u32 {
    let (a, b, c) = (u64::from(a), u64::from(b), u64::from(c));
    ((a * b + c / 2) / c) as u32
}

/// The rectangle of `output` that `content` scales into: as large as fits,
/// with the content's aspect ratio kept, centred, with even dimensions and
/// even offsets. What is outside it is the black bars.
///
/// Content wider than the output fills its width, with bars above and below;
/// taller content fills its height, with bars left and right; the same aspect
/// fills the whole frame. It scales up as well as down, as libobs does, so a
/// window that shrinks mid-game still fills the frame rather than sitting in
/// a corner of it.
///
/// An empty content or output, or content so thin that its scaled side
/// rounds to nothing, gives an empty rectangle: there is nothing to draw.
pub fn letterbox(content: Size, output: Size) -> Rect {
    if content.is_empty() || output.is_empty() {
        return Rect::default();
    }
    let (cw, ch) = (u64::from(content.width), u64::from(content.height));
    let (ow, oh) = (u64::from(output.width), u64::from(output.height));
    // Cross-multiplied, so the comparison is exact.
    let (width, height) = if cw * oh >= ch * ow {
        (output.width, scale(output.width, content.height, content.width))
    } else {
        (scale(output.height, content.width, content.height), output.height)
    };
    let width = even(width.min(output.width));
    let height = even(height.min(output.height));
    if width == 0 || height == 0 {
        return Rect::default();
    }
    Rect {
        left: even((output.width - width) / 2),
        top: even((output.height - height) / 2),
        width,
        height,
    }
}

/// What to do with one captured frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// The content is the output's size, give or take the odd pixel the
    /// output was rounded down by: copy the output's worth from the top-left,
    /// as before. The common case, and a plain GPU copy.
    Copy,
    /// The content is some other size: scale it into this rectangle, black
    /// around it.
    Scale(Rect),
    /// Nothing usable: no content (a minimised window), or content larger
    /// than the texture it came in, which is a frame from before the pool was
    /// recreated at the new size and has only part of the picture. The tick
    /// repeats the last frame instead.
    Skip,
}

/// Decides [`Placement`] for a frame whose content is `content`, delivered in
/// a texture of `texture`, going into an output frame of `output`.
pub fn place(content: Size, texture: Size, output: Size) -> Placement {
    if content.is_empty()
        || output.is_empty()
        || content.width > texture.width
        || content.height > texture.height
    {
        return Placement::Skip;
    }
    if even(content.width) == output.width && even(content.height) == output.height {
        return Placement::Copy;
    }
    let rect = letterbox(content, output);
    if rect.is_empty() { Placement::Skip } else { Placement::Scale(rect) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: Size = Size::new(1920, 1080);

    fn rect(left: u32, top: u32, width: u32, height: u32) -> Rect {
        Rect { left, top, width, height }
    }

    #[test]
    fn the_same_aspect_fills_the_frame() {
        assert_eq!(letterbox(Size::new(1280, 720), HD), rect(0, 0, 1920, 1080));
        assert_eq!(letterbox(Size::new(3840, 2160), HD), rect(0, 0, 1920, 1080));
        assert_eq!(letterbox(HD, HD), rect(0, 0, 1920, 1080));
    }

    #[test]
    fn wider_content_is_letterboxed_top_and_bottom() {
        // 21:9 into 16:9: 1920 x 810, 270 rows of bars split 135/135, and the
        // odd offset goes down to 134.
        let r = letterbox(Size::new(2560, 1080), HD);
        assert_eq!(r, rect(0, 134, 1920, 810));
        assert_eq!(r.right(), 1920);
        assert!(r.bottom() <= 1080);
    }

    #[test]
    fn taller_content_is_pillarboxed_left_and_right() {
        // 5:4 into 16:9: 1350 x 1080, centred, 284 columns of bar on the left.
        assert_eq!(letterbox(Size::new(1280, 1024), HD), rect(284, 0, 1350, 1080));
        // 4:3.
        assert_eq!(letterbox(Size::new(1024, 768), HD), rect(240, 0, 1440, 1080));
    }

    #[test]
    fn smaller_content_scales_up_and_larger_scales_down() {
        assert_eq!(letterbox(Size::new(800, 600), HD), rect(240, 0, 1440, 1080));
        assert_eq!(letterbox(Size::new(3200, 2400), HD), rect(240, 0, 1440, 1080));
        // A tall window into a wide frame, and the other way round.
        assert_eq!(letterbox(Size::new(1080, 1920), HD), rect(656, 0, 608, 1080));
        assert_eq!(letterbox(HD, Size::new(1080, 1920)), rect(0, 656, 1080, 608));
    }

    #[test]
    fn odd_sizes_give_even_dimensions_and_offsets() {
        let cases = [
            (Size::new(1921, 1081), HD),
            (Size::new(1279, 721), HD),
            (Size::new(1000, 999), HD),
            (Size::new(1920, 1080), Size::new(1919, 1079)),
            (Size::new(333, 777), Size::new(1281, 721)),
        ];
        for (content, output) in cases {
            let r = letterbox(content, output);
            assert!(!r.is_empty(), "{content:?} into {output:?}");
            for v in [r.left, r.top, r.width, r.height] {
                assert_eq!(v % 2, 0, "{content:?} into {output:?} gave {r:?}");
            }
            assert!(r.right() <= output.width && r.bottom() <= output.height, "{r:?}");
        }
    }

    #[test]
    fn zero_and_degenerate_sizes_give_nothing_to_draw() {
        assert!(letterbox(Size::new(0, 720), HD).is_empty());
        assert!(letterbox(Size::new(1280, 0), HD).is_empty());
        assert!(letterbox(Size::new(0, 0), HD).is_empty());
        assert!(letterbox(Size::new(1280, 720), Size::new(0, 1080)).is_empty());
        assert!(letterbox(Size::new(1280, 720), Size::new(1, 1)).is_empty());
        // So thin that the scaled height rounds to nothing.
        assert!(letterbox(Size::new(100_000, 1), HD).is_empty());
        // A single pixel still fills a frame: scaled, not dropped.
        assert_eq!(letterbox(Size::new(1, 1), HD), rect(420, 0, 1080, 1080));
    }

    #[test]
    fn negative_wgc_sizes_are_empty() {
        assert!(Size::from_i32(-1, 720).is_empty());
        assert_eq!(Size::from_i32(1280, 720), Size::new(1280, 720));
    }

    /// Over a spread of sizes: inside the frame, even, centred to within the
    /// even rounding, touching two opposite edges, and the aspect ratio kept
    /// to within the rounding of one side.
    #[test]
    fn every_fit_is_inside_centred_and_keeps_its_aspect() {
        let sides = [2, 3, 17, 240, 480, 719, 720, 1024, 1080, 1366, 1440, 1920, 2560, 3840];
        let outputs = [HD, Size::new(1280, 720), Size::new(2560, 1080), Size::new(1024, 768)];
        for &output in &outputs {
            for &w in &sides {
                for &h in &sides {
                    let content = Size::new(w, h);
                    let r = letterbox(content, output);
                    if r.is_empty() {
                        continue;
                    }
                    assert!(r.right() <= output.width && r.bottom() <= output.height);
                    for v in [r.left, r.top, r.width, r.height] {
                        assert!(v.is_multiple_of(2), "{content:?} into {output:?}: {r:?}");
                    }
                    // Centred: the two bars differ by the rounding, at most 3.
                    let (l, rr) = (r.left, output.width - r.right());
                    let (t, b) = (r.top, output.height - r.bottom());
                    assert!(l.abs_diff(rr) <= 3 && t.abs_diff(b) <= 3, "{content:?} {r:?}");
                    // Fills one axis, to within the even rounding.
                    assert!(
                        output.width - r.width <= 1 || output.height - r.height <= 1,
                        "{content:?} into {output:?} fills neither axis: {r:?}"
                    );
                    // Aspect: r.w / r.h against w / h, compared as the error
                    // in pixels on the side that was computed.
                    let expect_h = f64::from(r.width) * f64::from(h) / f64::from(w);
                    let expect_w = f64::from(r.height) * f64::from(w) / f64::from(h);
                    let off = (expect_h - f64::from(r.height))
                        .abs()
                        .min((expect_w - f64::from(r.width)).abs());
                    assert!(off <= 2.0, "{content:?} into {output:?}: {r:?} is off by {off}");
                }
            }
        }
    }

    #[test]
    fn the_output_size_is_copied_and_anything_else_scaled() {
        assert_eq!(place(HD, HD, HD), Placement::Copy);
        // The output was rounded down from an odd window, and still is.
        assert_eq!(place(Size::new(1921, 1081), Size::new(1921, 1081), HD), Placement::Copy);
        // A pool texture larger than the content, as while WGC catches up.
        assert_eq!(place(HD, Size::new(2560, 1440), HD), Placement::Copy);
        assert_eq!(
            place(Size::new(1280, 1024), Size::new(1280, 1024), HD),
            Placement::Scale(rect(284, 0, 1350, 1080))
        );
        assert_eq!(
            place(Size::new(1280, 720), Size::new(1280, 720), HD),
            Placement::Scale(rect(0, 0, 1920, 1080))
        );
    }

    #[test]
    fn nothing_usable_is_skipped() {
        // Minimised: WGC's content has no area.
        assert_eq!(place(Size::new(0, 0), HD, HD), Placement::Skip);
        // Content larger than its texture: from before the pool was recreated.
        assert_eq!(place(Size::new(2560, 1440), HD, HD), Placement::Skip);
        assert_eq!(place(Size::new(100_000, 1), Size::new(100_000, 1), HD), Placement::Skip);
        assert_eq!(place(HD, HD, Size::new(0, 0)), Placement::Skip);
    }
}
