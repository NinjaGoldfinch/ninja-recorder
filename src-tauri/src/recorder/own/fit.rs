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
//! **Only the client area goes in** (#314). WGC captures the whole window,
//! and a game in Windowed mode has a title bar and borders around the part it
//! draws. [`client_insets`] measures that frame from the window's rectangles,
//! and [`source_rect`] takes it off each captured frame, so what is copied or
//! scaled is the game's picture alone. A borderless or fullscreen window has
//! no frame, and its source is the whole frame, as before.
//!
//! Pure, so the maths is tested on every platform; `win::scale` does what it
//! says with the D3D11 video processor, and `win::capture::client_insets`
//! reads the rectangles from Windows.

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

    /// Too small in either dimension to be a picture of the game: see
    /// [`MIN_CONTENT`]. An empty size is too small too.
    pub fn is_too_small(self) -> bool {
        self.width < MIN_CONTENT || self.height < MIN_CONTENT
    }
}

/// The smallest content, in pixels on either side, that is treated as a
/// picture of the game rather than as a window with nothing to show.
///
/// When League is minimised, or alt-tabbed out of fullscreen, WGC does not
/// always stop sending frames: it can send some whose content is 1x1 first
/// (#301). Scaled like any other size, that single pixel fills a 1440x1440
/// square in a 2560x1440 recording, a solid box for as long as it lasts. No
/// game frame is anywhere near this small, so anything under it is skipped
/// and the ticks hold the last real picture, which is what a minimised window
/// that sends no frames gets anyway. 64 is far below any window the game can
/// be played in, and far enough above 1 to catch whatever WGC reports for a
/// window on its way to the taskbar.
pub const MIN_CONTENT: u32 = 64;

/// A rectangle inside a frame: the output, or the part of a captured frame
/// that is the picture.
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

    pub fn size(self) -> Size {
        Size::new(self.width, self.height)
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
    /// output was rounded down by: copy the output's worth from the source's
    /// top-left (the client area's, in Windowed mode). The common case, and a
    /// plain GPU copy.
    Copy,
    /// The content is some other size: scale it into this rectangle, black
    /// around it.
    Scale(Rect),
    /// Nothing usable: no content, or content under [`MIN_CONTENT`] (both a
    /// minimised or alt-tabbed window), or content larger
    /// than the texture it came in, which is a frame from before the pool was
    /// recreated at the new size and has only part of the picture. The tick
    /// repeats the last frame instead.
    Skip,
}

/// Decides [`Placement`] for a frame whose picture is `source` (see
/// [`source_rect`]), delivered in a texture of `texture`, going into an output
/// frame of `output`. [`Placement::Copy`] copies the output's worth from
/// `source`'s top-left corner.
pub fn place(source: Rect, texture: Size, output: Size) -> Placement {
    let content = source.size();
    if content.is_too_small()
        || output.is_empty()
        || u64::from(source.left) + u64::from(source.width) > u64::from(texture.width)
        || u64::from(source.top) + u64::from(source.height) > u64::from(texture.height)
    {
        return Placement::Skip;
    }
    if even(content.width) == output.width && even(content.height) == output.height {
        return Placement::Copy;
    }
    let rect = letterbox(content, output);
    if rect.is_empty() { Placement::Skip } else { Placement::Scale(rect) }
}

/// A rectangle as Windows reports one in screen coordinates: `right` and
/// `bottom` exclusive, and any of it negative on a monitor left of or above
/// the primary one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// How far the client area sits in from each edge of the captured window: the
/// title bar and borders of a window in Windowed mode, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Insets {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// The frame around `client` inside `window`, both in screen coordinates:
/// `window` the rectangle a WGC frame of the window covers (its extended
/// frame bounds), `client` the client area.
///
/// `None` means "take the whole frame": a window with no frame (borderless or
/// fullscreen, where the two rectangles are the same), and anything that
/// does not describe one non-empty rectangle inside another, which is what a
/// minimised, closing or mid-change window can report. A client edge outside
/// the window is clipped to it rather than refused: that side simply has no
/// border.
///
/// Insets rather than a rectangle because a frame's thickness does not change
/// when the window is resized, so frames of a new size that arrive before the
/// next measurement still lose the right strips.
pub fn client_insets(window: ScreenRect, client: ScreenRect) -> Option<Insets> {
    let span = |low: i32, high: i32| {
        u32::try_from(i64::from(high) - i64::from(low)).ok().filter(|&v| v > 0)
    };
    let width = span(window.left, window.right)?;
    let height = span(window.top, window.bottom)?;
    span(client.left, client.right)?;
    span(client.top, client.bottom)?;
    // How far `inner` is inside `outer`, or none if it is not.
    let inset =
        |outer: i32, inner: i32| u32::try_from(i64::from(inner) - i64::from(outer)).unwrap_or(0);
    let insets = Insets {
        left: inset(window.left, client.left),
        top: inset(window.top, client.top),
        right: inset(client.right, window.right),
        bottom: inset(client.bottom, window.bottom),
    };
    if u64::from(insets.left) + u64::from(insets.right) >= u64::from(width)
        || u64::from(insets.top) + u64::from(insets.bottom) >= u64::from(height)
    {
        return None;
    }
    (insets != Insets::default()).then_some(insets)
}

/// The part of a captured frame whose content is `content` that goes into the
/// recording: all of it less `insets`. The whole frame when there are no
/// insets, and when taking them off would leave less than a picture
/// ([`MIN_CONTENT`]) or nothing at all. A frame that is not a picture itself
/// is returned whole, so [`place`] skips it as it always has.
pub fn source_rect(content: Size, insets: Option<Insets>) -> Rect {
    let whole = Rect { left: 0, top: 0, width: content.width, height: content.height };
    let Some(i) = insets else {
        return whole;
    };
    if content.is_too_small() {
        return whole;
    }
    let width = content.width.checked_sub(i.left).and_then(|w| w.checked_sub(i.right));
    let height = content.height.checked_sub(i.top).and_then(|h| h.checked_sub(i.bottom));
    match (width, height) {
        (Some(width), Some(height)) if !Size::new(width, height).is_too_small() => {
            Rect { left: i.left, top: i.top, width, height }
        }
        _ => whole,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: Size = Size::new(1920, 1080);

    fn rect(left: u32, top: u32, width: u32, height: u32) -> Rect {
        Rect { left, top, width, height }
    }

    /// The whole of a frame of `content`: what `place` is handed when there
    /// is no client area to crop to.
    fn whole(content: Size) -> Rect {
        source_rect(content, None)
    }

    fn screen(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect { left, top, right, bottom }
    }

    fn insets(left: u32, top: u32, right: u32, bottom: u32) -> Insets {
        Insets { left, top, right, bottom }
    }

    /// A Windowed-mode window at 1920x1080 client, as Windows 11 draws one at
    /// 100%: a 31 px title bar and nothing at the sides once the invisible
    /// resize borders are left out, which the extended frame bounds do.
    const TITLED: (ScreenRect, ScreenRect) = (
        ScreenRect { left: 100, top: 50, right: 2020, bottom: 1161 },
        ScreenRect { left: 100, top: 81, right: 2020, bottom: 1161 },
    );

    #[test]
    fn a_title_bar_is_an_inset_at_the_top() {
        let (window, client) = TITLED;
        assert_eq!(client_insets(window, client), Some(insets(0, 31, 0, 0)));
        // Moving the window changes neither.
        let shift = |r: ScreenRect| screen(r.left - 700, r.top + 300, r.right - 700, r.bottom + 300);
        assert_eq!(client_insets(shift(window), shift(client)), Some(insets(0, 31, 0, 0)));
    }

    #[test]
    fn borders_on_every_side_and_negative_coordinates() {
        // A thick-framed window on a monitor left of and above the primary.
        let window = screen(-1930, -1090, -10, -2);
        let client = screen(-1922, -1059, -18, -10);
        assert_eq!(client_insets(window, client), Some(insets(8, 31, 8, 8)));
    }

    #[test]
    fn a_window_with_no_frame_has_no_insets() {
        // Borderless and fullscreen: the client area is the window.
        let r = screen(0, 0, 2560, 1440);
        assert_eq!(client_insets(r, r), None);
        let r = screen(-2560, 0, 0, 1440);
        assert_eq!(client_insets(r, r), None);
    }

    #[test]
    fn a_client_edge_outside_the_window_is_no_border_on_that_side() {
        let window = screen(0, 0, 1920, 1111);
        // One pixel past the left and the bottom, as rounding can leave it.
        let client = screen(-1, 31, 1920, 1112);
        assert_eq!(client_insets(window, client), Some(insets(0, 31, 0, 0)));
    }

    #[test]
    fn degenerate_rectangles_have_no_insets() {
        let fine = screen(0, 0, 1920, 1080);
        for (window, client) in [
            // Empty, and inverted, either rectangle.
            (screen(0, 0, 0, 0), fine),
            (fine, screen(10, 10, 10, 500)),
            (fine, screen(10, 10, 500, 10)),
            (screen(100, 0, 50, 1080), fine),
            (fine, screen(0, 500, 1920, 400)),
            // A minimised window's: off screen, with an empty client area.
            (screen(-32000, -32000, -31840, -31972), screen(-32000, -32000, -32000, -32000)),
            // Insets that meet or cross: nothing of the window is left.
            (fine, screen(5000, 0, 6000, 1080)),
            (fine, screen(0, -500, 1920, -100)),
        ] {
            assert_eq!(client_insets(window, client), None, "{window:?} / {client:?}");
        }
        // Spans past i32 are measured in i64, not wrapped or panicked on.
        let huge = screen(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        let got = client_insets(huge, fine).expect("a client area inside a huge window");
        assert_eq!((got.left, got.top), (1 << 31, 1 << 31));
        assert_eq!(client_insets(fine, huge), None);
    }

    #[test]
    fn the_source_is_the_frame_less_its_insets() {
        let content = Size::new(1920, 1111);
        let i = Some(insets(0, 31, 0, 0));
        assert_eq!(source_rect(content, i), rect(0, 31, 1920, 1080));
        assert_eq!(source_rect(content, None), rect(0, 0, 1920, 1111));
        // Every side, and odd sizes as a scaled display gives them.
        let i = Some(insets(9, 45, 9, 9));
        assert_eq!(source_rect(Size::new(1939, 1135), i), rect(9, 45, 1921, 1081));
    }

    #[test]
    fn a_resized_frame_keeps_the_same_insets() {
        // Measured at 1920x1111; the next frame is already 1280x751.
        let i = Some(insets(0, 31, 0, 0));
        assert_eq!(source_rect(Size::new(1280, 751), i), rect(0, 31, 1280, 720));
    }

    #[test]
    fn insets_that_leave_no_picture_fall_back_to_the_whole_frame() {
        let i = Some(insets(0, 31, 0, 0));
        // A frame that is not a picture itself stays whole, so place skips it.
        assert_eq!(source_rect(Size::new(1, 1), i), rect(0, 0, 1, 1));
        assert_eq!(source_rect(Size::new(0, 0), i), rect(0, 0, 0, 0));
        assert_eq!(place(source_rect(Size::new(1, 1), i), HD, HD), Placement::Skip);
        // Insets larger than the frame, or leaving under MIN_CONTENT.
        assert_eq!(
            source_rect(Size::new(100, 100), Some(insets(0, 200, 0, 0))),
            whole(Size::new(100, 100))
        );
        assert_eq!(
            source_rect(Size::new(200, 100), Some(insets(0, 40, 0, 0))),
            whole(Size::new(200, 100))
        );
        assert_eq!(
            source_rect(Size::new(200, 200), Some(insets(100, 0, 100, 0))),
            whole(Size::new(200, 200))
        );
        // Exactly MIN_CONTENT left is still a picture.
        let at = Size::new(MIN_CONTENT + 31, MIN_CONTENT);
        assert_eq!(
            source_rect(Size::new(MIN_CONTENT, MIN_CONTENT + 31), i),
            rect(0, 31, MIN_CONTENT, MIN_CONTENT)
        );
        assert_eq!(source_rect(at, Some(insets(31, 0, 0, 0))), rect(31, 0, MIN_CONTENT, MIN_CONTENT));
    }

    /// The case #314 is about: the recording is the client size, so a
    /// Windowed frame is a copy from under the title bar, not a scale of the
    /// whole window with its title bar in it.
    #[test]
    fn a_windowed_frame_is_copied_from_its_client_area() {
        let (window, client) = TITLED;
        let content = Size::new(1920, 1111);
        let source = source_rect(content, client_insets(window, client));
        assert_eq!(source, rect(0, 31, 1920, 1080));
        assert_eq!(place(source, content, HD), Placement::Copy);
        // Without the crop it would have been scaled, title bar and all.
        assert_eq!(place(whole(content), content, HD), Placement::Scale(rect(26, 0, 1866, 1080)));
        // Resized to 1280x720 client, before the insets are measured again.
        let content = Size::new(1280, 751);
        let source = source_rect(content, client_insets(window, client));
        assert_eq!(place(source, content, HD), Placement::Scale(rect(0, 0, 1920, 1080)));
    }

    #[test]
    fn a_source_outside_its_texture_is_skipped() {
        // A crop measured against a larger window, on a frame from a pool not
        // yet recreated: the source runs off the texture.
        assert_eq!(place(rect(0, 31, 1920, 1080), Size::new(1920, 1080), HD), Placement::Skip);
        assert_eq!(place(rect(8, 0, 1920, 1080), Size::new(1920, 1080), HD), Placement::Skip);
        assert_eq!(place(rect(0, 31, 1920, 1080), Size::new(1920, 1111), HD), Placement::Copy);
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
        assert_eq!(place(whole(HD), HD, HD), Placement::Copy);
        // The output was rounded down from an odd window, and still is.
        assert_eq!(place(whole(Size::new(1921, 1081)), Size::new(1921, 1081), HD), Placement::Copy);
        // A pool texture larger than the content, as while WGC catches up.
        assert_eq!(place(whole(HD), Size::new(2560, 1440), HD), Placement::Copy);
        assert_eq!(
            place(whole(Size::new(1280, 1024)), Size::new(1280, 1024), HD),
            Placement::Scale(rect(284, 0, 1350, 1080))
        );
        assert_eq!(
            place(whole(Size::new(1280, 720)), Size::new(1280, 720), HD),
            Placement::Scale(rect(0, 0, 1920, 1080))
        );
    }

    #[test]
    fn nothing_usable_is_skipped() {
        // Minimised: WGC's content has no area.
        assert_eq!(place(whole(Size::new(0, 0)), HD, HD), Placement::Skip);
        // Content larger than its texture: from before the pool was recreated.
        assert_eq!(place(whole(Size::new(2560, 1440)), HD, HD), Placement::Skip);
        assert_eq!(place(whole(Size::new(100_000, 1)), Size::new(100_000, 1), HD), Placement::Skip);
        assert_eq!(place(whole(HD), HD, Size::new(0, 0)), Placement::Skip);
    }

    /// #301: a minimised or alt-tabbed window's 1x1 frame would otherwise be
    /// scaled into a 1440x1440 box in a 2560x1440 recording.
    #[test]
    fn a_minimised_windows_tiny_frame_is_skipped_not_boxed() {
        let qhd = Size::new(2560, 1440);
        // What letterbox alone does with it: the box the issue saw.
        assert_eq!(letterbox(Size::new(1, 1), qhd), rect(560, 0, 1440, 1440));
        // What place does instead.
        assert_eq!(place(whole(Size::new(1, 1)), qhd, qhd), Placement::Skip);
        assert_eq!(place(whole(Size::new(1, 1)), Size::new(1, 1), qhd), Placement::Skip);
        // Under the minimum in either dimension, however large the other.
        for content in [
            Size::new(MIN_CONTENT - 1, MIN_CONTENT - 1),
            Size::new(MIN_CONTENT - 1, 1440),
            Size::new(2560, MIN_CONTENT - 1),
            Size::new(2, 2),
        ] {
            assert!(content.is_too_small(), "{content:?}");
            assert_eq!(place(whole(content), qhd, qhd), Placement::Skip, "{content:?}");
        }
    }

    #[test]
    fn content_at_the_minimum_is_still_a_picture() {
        let at = Size::new(MIN_CONTENT, MIN_CONTENT);
        assert!(!at.is_too_small());
        assert_eq!(place(whole(at), at, HD), Placement::Scale(rect(420, 0, 1080, 1080)));
        // A small but real window is scaled as before.
        assert_eq!(
            place(whole(Size::new(640, 480)), Size::new(640, 480), HD),
            Placement::Scale(rect(240, 0, 1440, 1080))
        );
    }
}
