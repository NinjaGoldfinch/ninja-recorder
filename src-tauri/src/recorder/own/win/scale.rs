//! Captured frames into the fixed-size output slots, and the D3D11 video
//! processor that scales them (#240).
//!
//! Two pieces, deliberately separate:
//!
//! - [`Processor`] is the video processor and nothing else: one BGRA input
//!   size, one output size and format, and a blit of a source rectangle into
//!   a destination rectangle with everything outside it cleared to black. It
//!   knows nothing about WGC. #239 reuses it for BGRA → NV12, which is the
//!   same blit with an NV12 output format and an output colour space.
//! - [`Fitter`] is the capture's policy: [`fit::place`] decides, per frame,
//!   whether it is a plain copy (the window is the size the encoder was set
//!   up for), a scale (it is not), or nothing (a minimised window). It owns
//!   the `Processor` and builds it only when a resize first needs it.
//!
//! **Everything here runs on the session's device and immediate context**,
//! which the encoder shares through the DXGI device manager; the device is
//! multithread-protected (`device.rs`), and that covers the video context too.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEX2D_VPIV,
    D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIDEO_COLOR,
    D3D11_VIDEO_COLOR_0, D3D11_VIDEO_COLOR_RGBA, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT,
    D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11RenderTargetView, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::Interface;

use super::device::Device;
use crate::recorder::own::fit::{self, Placement, Rect, Size};
use crate::{info, warn};

/// How many size changes one recording logs before it stops saying so. A
/// drag of a windowed border is a new size every frame for as long as the
/// drag lasts; the first few say what happened, the rest are noise.
const SIZE_LINES: u32 = 12;

/// A texture's width and height.
pub fn texture_size(texture: &ID3D11Texture2D) -> Size {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `texture` is live and `desc` is a live out-parameter.
    unsafe { texture.GetDesc(&mut desc) };
    Size::new(desc.Width, desc.Height)
}

/// Clears `texture` to opaque black. The texture must have been created with
/// `D3D11_BIND_RENDER_TARGET`, as the slots are.
pub fn fill_black(device: &Device, texture: &ID3D11Texture2D) -> Result<(), String> {
    let mut view: Option<ID3D11RenderTargetView> = None;
    // SAFETY: `texture` is live on this device and is a render target; the
    // default view covers mip 0 in the texture's own format.
    unsafe { device.device.CreateRenderTargetView(texture, None, Some(&mut view)) }
        .map_err(|e| format!("CreateRenderTargetView failed: {e}"))?;
    let view = view.ok_or("CreateRenderTargetView returned nothing")?;
    // SAFETY: `view` is live on this device's immediate context.
    unsafe { device.context.ClearRenderTargetView(&view, &[0.0, 0.0, 0.0, 1.0]) };
    Ok(())
}

fn to_rect(r: Rect) -> RECT {
    RECT {
        left: r.left as i32,
        top: r.top as i32,
        right: r.right() as i32,
        bottom: r.bottom() as i32,
    }
}

// --- The video processor -------------------------------------------------------

/// The D3D11 video processor, set up for one input size, one output size and
/// one output format.
///
/// A size change means a new `Processor`: the enumerator it is built from is
/// described by both sizes, and drivers are entitled to size their internal
/// surfaces from that description.
pub struct Processor {
    video_device: ID3D11VideoDevice,
    context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    input: Size,
    output: Size,
    /// The one input view in use, for the texture it views.
    input_view: Option<(ID3D11Texture2D, ID3D11VideoProcessorInputView)>,
    /// One output view per target texture, which is one per slot.
    output_views: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
}

impl Processor {
    /// A processor reading BGRA textures of `input` and writing textures of
    /// `output` in `format`. Fails if the device has no video processor, or
    /// has one that cannot read BGRA or write `format`.
    pub fn new(
        device: &Device,
        input: Size,
        output: Size,
        format: DXGI_FORMAT,
    ) -> Result<Processor, String> {
        if input.is_empty() || output.is_empty() {
            return Err(format!("no video processor for {input:?} into {output:?}"));
        }
        let video_device: ID3D11VideoDevice = device.device.cast().map_err(|e| {
            format!("the device has no video support (ID3D11VideoDevice): {e}")
        })?;
        let context: ID3D11VideoContext = device
            .context
            .cast()
            .map_err(|e| format!("the context has no ID3D11VideoContext: {e}"))?;
        let rate = DXGI_RATIONAL { Numerator: super::session::FPS, Denominator: 1 };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: input.width,
            InputHeight: input.height,
            OutputFrameRate: rate,
            OutputWidth: output.width,
            OutputHeight: output.height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        // SAFETY: `desc` is a complete, live descriptor.
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&desc) }
            .map_err(|e| format!("CreateVideoProcessorEnumerator failed: {e}"))?;
        for (fmt, flag, what) in [
            (DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT, "read BGRA"),
            (format, D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT, "write the output format"),
        ] {
            // SAFETY: `enumerator` is live.
            let support = unsafe { enumerator.CheckVideoProcessorFormat(fmt) }
                .map_err(|e| format!("CheckVideoProcessorFormat({fmt:?}) failed: {e}"))?;
            if support & flag.0 as u32 == 0 {
                return Err(format!("the video processor cannot {what} ({fmt:?})"));
            }
        }
        // SAFETY: `enumerator` is live; rate conversion 0 is the one every
        // processor has, and no conversion is asked for.
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|e| format!("CreateVideoProcessor failed: {e}"))?;

        // The state that does not change blit to blit. Progressive in,
        // nothing "improved" by the driver (auto processing is its own
        // sharpening and denoising), and opaque black wherever the stream
        // does not reach: that is the bars.
        // SAFETY (each): `processor` is live and was made by this video
        // device; stream 0 is its only stream.
        unsafe {
            context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            )
        };
        // SAFETY: as above.
        unsafe { context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false) };
        let black = D3D11_VIDEO_COLOR {
            Anonymous: D3D11_VIDEO_COLOR_0 {
                RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 },
            },
        };
        // SAFETY: as above; `black` is live for the call.
        unsafe { context.VideoProcessorSetOutputBackgroundColor(&processor, false, &black) };
        // No target rectangle: the whole output surface is the target, so the
        // background fills all of it the stream does not cover.
        // SAFETY: as above.
        unsafe { context.VideoProcessorSetOutputTargetRect(&processor, false, None) };

        Ok(Processor {
            video_device,
            context,
            enumerator,
            processor,
            input,
            output,
            input_view: None,
            output_views: Vec::new(),
        })
    }

    /// The input size this processor was built for.
    pub fn input(&self) -> Size {
        self.input
    }

    /// Scales `from` of `source` into `to` of `target`, and clears the rest
    /// of `target` to black. `source` must be `input`-sized BGRA, `target`
    /// `output`-sized in the output format, both on this device, and `target`
    /// a render target.
    pub fn blit(
        &mut self,
        source: &ID3D11Texture2D,
        from: Rect,
        target: &ID3D11Texture2D,
        to: Rect,
    ) -> Result<(), String> {
        if from.right() > self.input.width
            || from.bottom() > self.input.height
            || to.right() > self.output.width
            || to.bottom() > self.output.height
        {
            return Err(format!(
                "blit out of bounds: {from:?} of {:?} into {to:?} of {:?}",
                self.input, self.output
            ));
        }
        let input = self.input_view(source)?;
        let output = self.output_view(target)?;
        let (from, to) = (to_rect(from), to_rect(to));
        // SAFETY (both): `processor` is live; stream 0; the rectangles lie
        // within the input and output sizes (checked above).
        unsafe {
            self.context.VideoProcessorSetStreamSourceRect(&self.processor, 0, true, Some(&from))
        };
        // SAFETY: as above.
        unsafe {
            self.context.VideoProcessorSetStreamDestRect(&self.processor, 0, true, Some(&to))
        };
        let streams = [D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(input)),
            ..Default::default()
        }];
        // SAFETY: the processor, the output view and the one stream's input
        // view are live on this device; no past or future frames.
        let result =
            unsafe { self.context.VideoProcessorBlt(&self.processor, &output, 0, &streams) };
        // The stream held its own reference to the input view; give it back.
        let [stream] = streams;
        drop(ManuallyDrop::into_inner(stream.pInputSurface));
        result.map_err(|e| format!("VideoProcessorBlt failed: {e}"))
    }

    fn input_view(
        &mut self,
        source: &ID3D11Texture2D,
    ) -> Result<ID3D11VideoProcessorInputView, String> {
        if let Some((texture, view)) = &self.input_view
            && texture == source
        {
            return Ok(view.clone());
        }
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
            },
        };
        let mut view: Option<ID3D11VideoProcessorInputView> = None;
        // SAFETY: `source` and the enumerator are live on this device; `desc`
        // is complete; `view` is a live out-parameter.
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                source,
                &self.enumerator,
                &desc,
                Some(&mut view),
            )
        }
        .map_err(|e| format!("CreateVideoProcessorInputView failed: {e}"))?;
        let view = view.ok_or("CreateVideoProcessorInputView returned nothing")?;
        self.input_view = Some((source.clone(), view.clone()));
        Ok(view)
    }

    fn output_view(
        &mut self,
        target: &ID3D11Texture2D,
    ) -> Result<ID3D11VideoProcessorOutputView, String> {
        if let Some((_, view)) = self.output_views.iter().find(|(t, _)| t == target) {
            return Ok(view.clone());
        }
        let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut view: Option<ID3D11VideoProcessorOutputView> = None;
        // SAFETY: `target` and the enumerator are live on this device; `desc`
        // is complete; `view` is a live out-parameter.
        unsafe {
            self.video_device.CreateVideoProcessorOutputView(
                target,
                &self.enumerator,
                &desc,
                Some(&mut view),
            )
        }
        .map_err(|e| format!("CreateVideoProcessorOutputView failed: {e}"))?;
        let view = view.ok_or("CreateVideoProcessorOutputView returned nothing")?;
        // The slots are a fixed set, so this stays as small as they are; the
        // bound is for a caller that keeps handing over new textures.
        if self.output_views.len() >= 32 {
            self.output_views.clear();
        }
        self.output_views.push((target.clone(), view.clone()));
        Ok(view)
    }
}

// --- The capture's policy --------------------------------------------------

/// What [`Fitter::place`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    /// Copied: the content is the output's size.
    Copied,
    /// Scaled into the output, letterboxed.
    Scaled,
    /// Scaled could not be, so copied from the top-left over black: a window
    /// that grew is cropped, one that shrank sits in a black frame. What the
    /// backend falls back to when the device has no usable video processor.
    Cropped,
    /// Not used; the slot is untouched and the tick repeats the last frame.
    Skipped,
}

/// Puts WGC's frames into output slots of one fixed size, following the game
/// window through resizes (see `fit`).
pub struct Fitter {
    output: Size,
    /// Our own copy of a frame that needs scaling, at the frame's texture
    /// size. The video processor reads this, never WGC's texture: WGC's pool
    /// is recreated on a resize and recycles its buffers as soon as a frame
    /// is dropped, while this is ours, stable, and one input view serves it.
    staging: Option<ID3D11Texture2D>,
    processor: Option<Processor>,
    /// Why the video processor could not be built, once it has failed; the
    /// fallback is then [`Placed::Cropped`] for the rest of the recording.
    unavailable: Option<String>,
    /// The last content size seen, for the log.
    last: Option<Size>,
    lines: u32,
}

impl Fitter {
    pub fn new(output: Size) -> Fitter {
        Fitter { output, staging: None, processor: None, unavailable: None, last: None, lines: 0 }
    }

    /// Puts `source`, whose picture is the top-left `content` of it, into
    /// `slot`, which is output-sized. [`Placed::Skipped`] leaves the slot as
    /// it was.
    pub fn place(
        &mut self,
        device: &Device,
        source: &ID3D11Texture2D,
        content: Size,
        slot: &ID3D11Texture2D,
    ) -> Result<Placed, String> {
        let texture = texture_size(source);
        let placement = fit::place(content, texture, self.output);
        self.note(content, placement);
        match placement {
            Placement::Skip => Ok(Placed::Skipped),
            Placement::Copy => {
                copy_top_left(device, slot, source, self.output);
                Ok(Placed::Copied)
            }
            Placement::Scale(to) => {
                if self.unavailable.is_none() {
                    match self.scale(device, source, texture, content, slot, to) {
                        Ok(()) => return Ok(Placed::Scaled),
                        // A lost device is not the processor's fault: the
                        // session's loop sees it and ends the recording.
                        Err(e) if super::device::removed(device).is_some() => return Err(e),
                        Err(e) => {
                            warn!(
                                "recorder",
                                "own backend: cannot scale a resized window ({e}); cropping \
                                 instead for the rest of this recording"
                            );
                            self.unavailable = Some(e);
                            self.processor = None;
                            self.staging = None;
                        }
                    }
                }
                fill_black(device, slot)?;
                let w = content.width.min(self.output.width);
                let h = content.height.min(self.output.height);
                copy_top_left(device, slot, source, Size::new(w, h));
                Ok(Placed::Cropped)
            }
        }
    }

    fn scale(
        &mut self,
        device: &Device,
        source: &ID3D11Texture2D,
        texture: Size,
        content: Size,
        slot: &ID3D11Texture2D,
        to: Rect,
    ) -> Result<(), String> {
        let staging = match &self.staging {
            Some(staging) if texture_size(staging) == texture => staging.clone(),
            _ => {
                let staging = create_bgra(device, texture)?;
                self.staging = Some(staging.clone());
                staging
            }
        };
        // SAFETY: both textures are live on this device, the same size and
        // the same format (both BGRA; the staging copy is made from the
        // source's size above).
        unsafe { device.context.CopyResource(&staging, source) };
        if self.processor.as_ref().is_none_or(|p| p.input() != texture) {
            self.processor = None;
            self.processor =
                Some(Processor::new(device, texture, self.output, DXGI_FORMAT_B8G8R8A8_UNORM)?);
        }
        let processor = self.processor.as_mut().expect("just built");
        let from = Rect { left: 0, top: 0, width: content.width, height: content.height };
        processor.blit(&staging, from, slot, to)
    }

    /// Logs a change of content size, up to [`SIZE_LINES`] times.
    fn note(&mut self, content: Size, placement: Placement) {
        if self.last == Some(content) || content.is_empty() {
            return;
        }
        let first = self.last.is_none();
        self.last = Some(content);
        if first && placement == Placement::Copy {
            return;
        }
        self.lines += 1;
        if self.lines > SIZE_LINES {
            return;
        }
        let what = match placement {
            Placement::Copy => "copied as is".to_string(),
            Placement::Scale(r) => format!(
                "scaled to {}x{} at ({}, {}), black around it",
                r.width, r.height, r.left, r.top
            ),
            Placement::Skip => "skipped until WGC catches up".to_string(),
        };
        let (w, h) = (self.output.width, self.output.height);
        info!(
            "recorder",
            "own backend: the game window is now {}x{}; into the {w}x{h} recording it is {what}{}",
            content.width,
            content.height,
            if self.lines == SIZE_LINES { " (further size changes are not logged)" } else { "" }
        );
    }
}

/// Copies the top-left `size` of `source` into the top-left of `slot`. The
/// caller keeps `size` within both.
fn copy_top_left(device: &Device, slot: &ID3D11Texture2D, source: &ID3D11Texture2D, size: Size) {
    let region =
        D3D11_BOX { left: 0, top: 0, front: 0, right: size.width, bottom: size.height, back: 1 };
    // SAFETY: both textures are live on this device and share a format, and
    // the box lies within both (`fit::place` checked the source; the size is
    // at most the output's, which is the slot's).
    unsafe { device.context.CopySubresourceRegion(slot, 0, 0, 0, 0, source, 0, Some(&region)) };
}

/// A default-usage BGRA texture the video processor can read and write.
pub fn create_bgra(device: &Device, size: Size) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size.width,
        Height: size.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    // SAFETY: `desc` is a live, fully initialised descriptor; no initial data.
    unsafe { device.device.CreateTexture2D(&desc, None, Some(&mut texture)) }
        .map_err(|e| format!("CreateTexture2D failed: {e}"))?;
    texture.ok_or_else(|| "CreateTexture2D returned nothing".to_string())
}
