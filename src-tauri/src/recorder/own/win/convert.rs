//! Each tick's frame as the H.264 encoder takes it: NV12, as a texture or in
//! system memory (#239).
//!
//! The capture puts BGRA into the slots (`capture.rs`, `scale.rs`); the
//! encoders take NV12. How a frame gets there depends on the encoder and the
//! device, and there are three ways, chosen once per recording:
//!
//! | Encoder input | Device | Conversion |
//! |---|---|---|
//! | texture (hardware) | any GPU | the video processor, into an NV12 texture per slot |
//! | system memory (software) | has a video processor | the same, into one texture, read back |
//! | system memory (software) | no video processor | read the BGRA back, `own::nv12` on the CPU |
//!
//! The first is every recording on a machine with a hardware encoder, and
//! costs one GPU blit per new frame. The third is what the CI runner does
//! (WARP has no video processor), and what a VM with no GPU would do.
//!
//! **A texture handed to a hardware encoder is the slot's own**, and the
//! sample wrapping it is tracked with the slot's callback, so
//! `Slot::busy` still says whether the encoder holds it. A slot the encoder
//! holds is never written (`session.rs` only fills free ones), and a slot is
//! converted only when a new frame has been placed in it, so the texture the
//! encoder is reading is never the one being converted into.
//!
//! **Converted once per frame, not once per tick.** The cadence loop repeats
//! the last frame when the game has not produced a new one, and a repeated
//! frame is the same NV12 again: the same texture, or the same bytes.

use std::sync::atomic::Ordering;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_VIDEO_ENCODER, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC,
};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFSample, MFCreateDXGISurfaceBuffer, MFCreateTrackedSample,
};
use windows::core::Interface;

use super::capture::Slot;
use super::device::Device;
use super::encode;
use super::h264::InputMode;
use super::scale::Processor;
use crate::recorder::own::fit::{Rect, Size};
use crate::recorder::own::nv12;

/// How [`Frames`] converts.
enum Conversion {
    /// The video processor, into `textures`.
    Gpu(Processor),
    /// Read back and `own::nv12`.
    Cpu,
}

/// NV12 frames for the encoder, made from the slots.
pub struct Frames {
    size: Size,
    input: InputMode,
    conversion: Conversion,
    /// Texture input: one NV12 texture per slot. System memory through the
    /// GPU: one, read back through `staging`.
    textures: Vec<ID3D11Texture2D>,
    /// CPU-readable: NV12 for the GPU path's read-back, BGRA for the CPU's.
    staging: Option<ID3D11Texture2D>,
    /// The slot whose frame was converted last.
    converted: Option<usize>,
    /// System memory: that frame, tightly packed.
    bytes: Vec<u8>,
}

fn texture_desc(size: Size, format: DXGI_FORMAT, staging: bool, bind: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: size.width,
        Height: size.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: if staging { D3D11_USAGE_STAGING } else { D3D11_USAGE_DEFAULT },
        BindFlags: bind,
        CPUAccessFlags: if staging { D3D11_CPU_ACCESS_READ.0 as u32 } else { 0 },
        MiscFlags: 0,
    }
}

fn create_texture(device: &Device, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D, String> {
    let mut texture: Option<ID3D11Texture2D> = None;
    // SAFETY: `desc` is a live, fully initialised descriptor; no initial data.
    unsafe { device.device.CreateTexture2D(desc, None, Some(&mut texture)) }
        .map_err(|e| format!("CreateTexture2D ({:?}) failed: {e}", desc.Format))?;
    texture.ok_or_else(|| "CreateTexture2D returned nothing".to_string())
}

/// An NV12 texture the video processor can write: a render target, and
/// marked for the video encoder where the driver allows it.
fn create_nv12(device: &Device, size: Size) -> Result<ID3D11Texture2D, String> {
    let with_encoder = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_VIDEO_ENCODER.0) as u32;
    create_texture(device, &texture_desc(size, DXGI_FORMAT_NV12, false, with_encoder)).or_else(
        |_| {
            let plain = D3D11_BIND_RENDER_TARGET.0 as u32;
            create_texture(device, &texture_desc(size, DXGI_FORMAT_NV12, false, plain))
        },
    )
}

impl Frames {
    /// Frames of `size` for an encoder taking `input`, over `slots` slots.
    /// Texture input needs the video processor; system memory falls back to
    /// the CPU without one.
    pub fn new(device: &Device, size: Size, slots: usize, input: InputMode) -> Result<Frames, String> {
        let processor = Processor::new(device, size, size, DXGI_FORMAT_NV12);
        let (conversion, textures, staging) = match (input, processor) {
            (InputMode::Texture, Ok(processor)) => {
                let textures =
                    (0..slots).map(|_| create_nv12(device, size)).collect::<Result<Vec<_>, _>>()?;
                (Conversion::Gpu(processor), textures, None)
            }
            (InputMode::Texture, Err(e)) => {
                return Err(format!(
                    "a hardware encoder needs frames converted on the GPU, and this device \
                     cannot: {e}"
                ));
            }
            (InputMode::Memory, Ok(processor)) => {
                let texture = create_nv12(device, size)?;
                let staging = create_texture(device, &texture_desc(size, DXGI_FORMAT_NV12, true, 0))?;
                (Conversion::Gpu(processor), vec![texture], Some(staging))
            }
            (InputMode::Memory, Err(_)) => {
                let staging =
                    create_texture(device, &texture_desc(size, DXGI_FORMAT_B8G8R8A8_UNORM, true, 0))?;
                (Conversion::Cpu, Vec::new(), Some(staging))
            }
        };
        Ok(Frames { size, input, conversion, textures, staging, converted: None, bytes: Vec::new() })
    }

    /// How frames are made, for the log.
    pub fn describe(&self) -> &'static str {
        match (self.input, &self.conversion) {
            (InputMode::Texture, _) => "NV12 textures from the video processor",
            (InputMode::Memory, Conversion::Gpu(_)) => {
                "NV12 in system memory, converted by the video processor and read back"
            }
            (InputMode::Memory, Conversion::Cpu) => {
                "NV12 in system memory, converted on the CPU (no video processor)"
            }
        }
    }

    /// The sample for one tick showing `slots[index]`, at `time` for
    /// `duration` (100 ns units). Converts the slot first if its frame has
    /// not been converted yet.
    pub fn sample(
        &mut self,
        device: &Device,
        slots: &[Slot],
        index: usize,
        time: i64,
        duration: i64,
    ) -> Result<IMFSample, String> {
        let slot = slots.get(index).ok_or("no such slot")?;
        if self.converted != Some(index) {
            self.convert(device, slot, index)?;
            self.converted = Some(index);
        }
        match self.input {
            InputMode::Texture => texture_sample(&self.textures[index], slot, time, duration),
            InputMode::Memory => encode::memory_sample(&self.bytes, time, duration),
        }
    }

    fn convert(&mut self, device: &Device, slot: &Slot, index: usize) -> Result<(), String> {
        let whole = Rect { left: 0, top: 0, width: self.size.width, height: self.size.height };
        match &mut self.conversion {
            Conversion::Gpu(processor) => {
                let target = match self.input {
                    InputMode::Texture => &self.textures[index],
                    InputMode::Memory => &self.textures[0],
                };
                processor.blit(&slot.texture, whole, target, whole)?;
                if self.input == InputMode::Memory {
                    let staging = self.staging.as_ref().ok_or("no staging texture")?;
                    // SAFETY: both live on this device, the same size and
                    // format (NV12).
                    unsafe { device.context.CopyResource(staging, target) };
                    self.bytes = read_nv12(device, staging, self.size)?;
                }
            }
            Conversion::Cpu => {
                let staging = self.staging.as_ref().ok_or("no staging texture")?;
                // SAFETY: both live on this device, the same size and format
                // (BGRA; the staging texture was made at the slots' size).
                unsafe { device.context.CopyResource(staging, &slot.texture) };
                self.bytes = map(device, staging, |data, pitch| {
                    let len = pitch * self.size.height as usize;
                    // SAFETY: a mapped texture is `pitch` bytes a row for its
                    // height, and it stays mapped until `map` unmaps it.
                    let bgra = unsafe { std::slice::from_raw_parts(data, len) };
                    nv12::bgra_to_nv12(bgra, pitch, self.size.width, self.size.height)
                        .ok_or_else(|| "the frame size is not even".to_string())
                })??;
            }
        }
        Ok(())
    }
}

/// Maps `staging` for reading, runs `read` over its data and row pitch, and
/// unmaps it.
fn map<T>(
    device: &Device,
    staging: &ID3D11Texture2D,
    read: impl FnOnce(*const u8, usize) -> T,
) -> Result<T, String> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: `staging` is CPU-readable on this device; `mapped` is a live
    // out-parameter. The map waits for the copy into it to finish.
    unsafe { device.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
        .map_err(|e| format!("Map failed: {e}"))?;
    let out = read(mapped.pData.cast::<u8>().cast_const(), mapped.RowPitch as usize);
    // SAFETY: mapped above, and `read` has returned.
    unsafe { device.context.Unmap(staging, 0) };
    Ok(out)
}

/// An NV12 staging texture's two planes, tightly packed. D3D11 maps a
/// planar texture as its luma rows, then its chroma rows at the same pitch,
/// starting `pitch * height` bytes in.
fn read_nv12(device: &Device, staging: &ID3D11Texture2D, size: Size) -> Result<Vec<u8>, String> {
    let (w, h) = (size.width as usize, size.height as usize);
    map(device, staging, |data, pitch| {
        let mut out = Vec::with_capacity(nv12::frame_len(size.width, size.height));
        for row in 0..h + h / 2 {
            // SAFETY: the mapping holds `h` luma rows and `h / 2` chroma rows
            // of `pitch` bytes each, every one at least `w` bytes, and stays
            // mapped until `map` unmaps it.
            let line = unsafe { std::slice::from_raw_parts(data.add(row * pitch), w) };
            out.extend_from_slice(line);
        }
        out
    })
}

/// `texture`, wrapped, not copied, in a sample tracked by `slot`'s callback,
/// so the slot knows when the encoder has let go of it.
fn texture_sample(
    texture: &ID3D11Texture2D,
    slot: &Slot,
    time: i64,
    duration: i64,
) -> Result<IMFSample, String> {
    // SAFETY: the texture is live; subresource 0, not bottom-up.
    let buffer = unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false) }
        .map_err(|e| format!("MFCreateDXGISurfaceBuffer failed: {e}"))?;
    // A DXGI buffer starts with a current length of zero, and some encoders
    // treat that as an empty frame. Its contiguous length is the real size.
    if let Ok(two_d) = buffer.cast::<IMF2DBuffer>()
        // SAFETY: `two_d` is live.
        && let Ok(length) = unsafe { two_d.GetContiguousLength() }
    {
        // SAFETY: `buffer` is live; the length is its own.
        let _ = unsafe { buffer.SetCurrentLength(length) };
    }
    // SAFETY: no arguments.
    let tracked = unsafe { MFCreateTrackedSample() }
        .map_err(|e| format!("MFCreateTrackedSample failed: {e}"))?;
    slot.in_flight.fetch_add(1, Ordering::AcqRel);
    // SAFETY: the callback is live for as long as the slot is, which outlives
    // every sample made from it (the encoder is shut down first).
    if let Err(e) = unsafe { tracked.SetAllocator(&slot.callback, None) } {
        slot.in_flight.fetch_sub(1, Ordering::AcqRel);
        return Err(format!("SetAllocator failed: {e}"));
    }
    let sample: IMFSample =
        tracked.cast().map_err(|e| format!("a tracked sample is not an IMFSample: {e}"))?;
    // SAFETY: `sample` and `buffer` are live.
    unsafe { sample.AddBuffer(&buffer) }.map_err(|e| format!("AddBuffer failed: {e}"))?;
    encode::set_times(&sample, time, duration)?;
    Ok(sample)
}
