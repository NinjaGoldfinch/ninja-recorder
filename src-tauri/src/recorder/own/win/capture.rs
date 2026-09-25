//! Windows.Graphics.Capture of the game window, and the textures its frames
//! are copied into. Ported from `spikes/p0c-video/src/win/video.rs` and the
//! capture half of its `record()`.
//!
//! **WGC reads what DWM composites; nothing here touches the game process.**
//! No injection, no hook, no handle to League beyond the window's HWND,
//! which is a hard constraint (DEVELOPMENT.md §1.1).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::Foundation::Metadata::ApiInformation;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess,
    GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Security::Authorization::AppCapabilityAccess::AppCapabilityAccessStatus;
use windows::Win32::Foundation::{E_NOTIMPL, HWND};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, IMFAsyncCallback_Impl, IMFAsyncResult,
};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{HSTRING, IInspectable, Interface, Ref, implement};

use super::device::Device;

/// WGC's own pool depth. Two is what the spike ran with: frames are copied
/// out of it at once, so a deeper pool would only add latency.
const POOL_BUFFERS: i32 = 2;
const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

// --- Slots ---------------------------------------------------------------

/// Counts a slot's samples back in: Media Foundation calls this when a
/// tracked sample's last reference goes, which is when the encoder has
/// finished reading the texture behind it.
#[implement(IMFAsyncCallback)]
struct Release(Arc<AtomicU32>);

impl IMFAsyncCallback_Impl for Release_Impl {
    fn GetParameters(&self, _flags: *mut u32, _queue: *mut u32) -> windows::core::Result<()> {
        // E_NOTIMPL is the documented "use the defaults".
        Err(E_NOTIMPL.into())
    }

    fn Invoke(&self, _result: Ref<IMFAsyncResult>) -> windows::core::Result<()> {
        self.0.fetch_sub(1, Ordering::AcqRel);
        Ok(())
    }
}

/// A texture of our own that one captured frame is copied into.
///
/// **Why copy at all.** WGC's frame pool has two buffers and recycles them as
/// soon as a frame is dropped, while the encoder reads its input
/// asynchronously. Wrapping the pool's texture directly means the compositor
/// can overwrite a frame the encoder has not read yet. The copy is GPU to
/// GPU and costs microseconds.
pub struct Slot {
    pub texture: ID3D11Texture2D,
    pub in_flight: Arc<AtomicU32>,
    pub callback: IMFAsyncCallback,
}

impl Slot {
    /// Whether the encoder still holds a sample made from this texture.
    pub fn busy(&self) -> bool {
        self.in_flight.load(Ordering::Acquire) > 0
    }
}

fn texture_desc(width: u32, height: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        // The video processor reads these as shader resources; render target
        // is what lets it treat them as a surface on some drivers.
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    }
}

pub fn create_slots(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    count: usize,
) -> Result<Vec<Slot>, String> {
    let desc = texture_desc(width, height);
    (0..count)
        .map(|_| {
            let mut texture: Option<ID3D11Texture2D> = None;
            // SAFETY: `desc` is a live, fully initialised descriptor; no
            // initial data.
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }
                .map_err(|e| format!("CreateTexture2D failed: {e}"))?;
            let in_flight = Arc::new(AtomicU32::new(0));
            Ok(Slot {
                texture: texture.ok_or("CreateTexture2D returned nothing")?,
                callback: Release(in_flight.clone()).into(),
                in_flight,
            })
        })
        .collect()
}

// --- The capture -----------------------------------------------------------

/// A WGC capture of one window: the item, a free-threaded frame pool on the
/// session's device, and the capture session itself.
///
/// Free-threaded, so frames are pulled with `TryGetNextFrame` from the
/// session thread rather than pushed to a dispatcher this process does not
/// have.
pub struct Capture {
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pool_size: SizeInt32,
    closed: Arc<AtomicBool>,
    closed_token: i64,
    /// What happened to WGC's yellow border, for the log.
    pub border: String,
}

impl Capture {
    /// Starts capturing `window`. The first frame is [`Capture::next_frame`]'s
    /// to wait for.
    pub fn start(device: &Device, window: HWND) -> Result<Capture, String> {
        // WGC's items are WinRT and the handles are Win32; the bridge is an
        // interop interface on the activation factory.
        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(|e| format!("no capture interop factory: {e}"))?;
        // SAFETY: `window` came from a live lookup; a window that has closed
        // since makes this fail with an error, not misbehave.
        let item = unsafe { interop.CreateForWindow::<GraphicsCaptureItem>(window) }
            .map_err(|e| format!("CreateForWindow failed: {e}"))?;
        let size = item.Size().map_err(|e| format!("the capture item has no size: {e}"))?;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device.winrt,
            PIXEL_FORMAT,
            POOL_BUFFERS,
            size,
        )
        .map_err(|e| format!("could not create the frame pool: {e}"))?;
        let closed = Arc::new(AtomicBool::new(false));
        let flag = closed.clone();
        let closed_token = item
            .Closed(&TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                flag.store(true, Ordering::Release);
                Ok(())
            }))
            .map_err(|e| format!("could not watch for the window closing: {e}"))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| format!("could not create the capture session: {e}"))?;
        let border = hide_border(&session);
        // The cursor is captured, as libobs's window capture does by default,
        // so a VOD shows where the player was pointing.
        let _ = session.SetIsCursorCaptureEnabled(true);
        session.StartCapture().map_err(|e| format!("StartCapture failed: {e}"))?;
        Ok(Capture { item, pool, session, pool_size: size, closed, closed_token, border })
    }

    /// The item's size when the capture started, which is what the encoder
    /// is set up for.
    pub fn size(&self) -> SizeInt32 {
        self.pool_size
    }

    /// Whether the window has gone: WGC raises `Closed` when the game ends
    /// or crashes. No frame comes after it.
    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The newest frame WGC has, discarding any older ones queued behind it,
    /// or `None` if nothing new has arrived.
    pub fn newest_frame(&self) -> Option<Direct3D11CaptureFrame> {
        let mut newest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            newest = Some(frame);
        }
        newest
    }

    /// The frame's texture, and the size of the content in it: the window's
    /// size when WGC took the frame, which after a resize is not the size of
    /// the texture it came in. A content size that differs from the pool's
    /// recreates the pool at the new size, as the spike did, so the frames
    /// after this one arrive whole. The encoder does not follow: `scale`
    /// fits whatever size this is into the one the recording started at.
    pub fn texture(
        &mut self,
        device: &Device,
        frame: &Direct3D11CaptureFrame,
    ) -> Result<(ID3D11Texture2D, SizeInt32), String> {
        let content = frame.ContentSize().unwrap_or(self.pool_size);
        if content.Width > 0
            && content.Height > 0
            && (content.Width != self.pool_size.Width || content.Height != self.pool_size.Height)
        {
            self.pool
                .Recreate(&device.winrt, PIXEL_FORMAT, POOL_BUFFERS, content)
                .map_err(|e| format!("could not resize the frame pool: {e}"))?;
            self.pool_size = content;
        }
        let surface = frame.Surface().map_err(|e| format!("frame has no surface: {e}"))?;
        let access: IDirect3DDxgiInterfaceAccess =
            surface.cast().map_err(|e| format!("surface is not a DXGI interface: {e}"))?;
        // SAFETY: `access` is live; the texture is a new reference we own.
        let texture = unsafe { access.GetInterface::<ID3D11Texture2D>() }
            .map_err(|e| format!("could not reach the texture: {e}"))?;
        Ok((texture, content))
    }

    /// Stops the capture. Also what `Drop` does, so a session that fails
    /// half-way up never leaves WGC capturing.
    fn close(&mut self) {
        let _ = self.session.Close();
        let _ = self.item.RemoveClosed(self.closed_token);
        let _ = self.pool.Close();
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.close();
    }
}

/// Turns WGC's yellow border off where this Windows supports it, and says
/// what happened (#219).
///
/// The border is drawn on screen only and never reaches the file, but the
/// libobs backend shows none, because libobs turns it off where Windows
/// allows; so does this, or the own backend would be a visible regression.
/// The same three steps as libobs's `winrt-capture.cpp`: the property only
/// exists from Windows build 20348, so ask `ApiInformation` first; request
/// `Borderless` access, which some builds require before the setter takes
/// effect; then clear the flag. Like libobs, the access status is reported
/// and not acted on. The flag is read back, so a setter that silently did
/// nothing shows as `on`.
fn hide_border(session: &GraphicsCaptureSession) -> String {
    let supported = ApiInformation::IsPropertyPresent(
        &HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureSession"),
        &HSTRING::from("IsBorderRequired"),
    )
    .unwrap_or(false);
    if !supported {
        return "on (this Windows cannot turn it off: IsBorderRequired needs build 20348+)"
            .to_string();
    }
    let access =
        match GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
            .and_then(|op| op.join())
        {
            Ok(status) => access_status_name(status).to_string(),
            Err(e) => format!("request failed: {e}"),
        };
    if let Err(e) = session.SetIsBorderRequired(false) {
        return format!("on (SetIsBorderRequired(false) failed: {e}; borderless access {access})");
    }
    match session.IsBorderRequired() {
        Ok(false) => format!("off (borderless access {access})"),
        Ok(true) => format!("on (the setter did not take; borderless access {access})"),
        Err(e) => format!("unknown (IsBorderRequired failed: {e}; borderless access {access})"),
    }
}

fn access_status_name(status: AppCapabilityAccessStatus) -> &'static str {
    match status {
        AppCapabilityAccessStatus::Allowed => "Allowed",
        AppCapabilityAccessStatus::DeniedBySystem => "DeniedBySystem",
        AppCapabilityAccessStatus::DeniedByUser => "DeniedByUser",
        AppCapabilityAccessStatus::NotDeclaredByApp => "NotDeclaredByApp",
        AppCapabilityAccessStatus::UserPromptRequired => "UserPromptRequired",
        _ => "unknown",
    }
}
