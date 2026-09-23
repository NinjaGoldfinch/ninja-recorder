//! GPUs, the D3D11 device, the WGC capture target, and the textures frames
//! are copied into.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Graphics::Capture::GraphicsCaptureItem;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice,
    IDXGIFactory1,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAsyncCallback, IMFAsyncCallback_Impl, IMFAsyncResult,
};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::{BOOL, Interface, Ref, implement, w};

use crate::Target;

// --- Adapters ------------------------------------------------------------

pub struct Adapter {
    pub index: u32,
    pub adapter: IDXGIAdapter1,
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub software: bool,
    pub vram_mb: u64,
}

/// The PCI vendor, and the encoder family Media Foundation would find on it.
pub fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10DE => "NVIDIA (NVENC)",
        0x1002 | 0x1022 => "AMD (AMF)",
        0x8086 => "Intel (Quick Sync)",
        0x1414 => "Microsoft (software rasterizer)",
        _ => "unknown vendor",
    }
}

pub fn adapters() -> Result<Vec<Adapter>, String> {
    // SAFETY: no arguments; the factory is a COM object owned by the result.
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|e| format!("CreateDXGIFactory1 failed: {e}"))?;
    let mut out = Vec::new();
    for index in 0.. {
        // SAFETY: `factory` is live; an index past the end is DXGI_ERROR_NOT_FOUND,
        // which ends the loop.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        // SAFETY: `adapter` is live.
        let desc = unsafe { adapter.GetDesc1() }.map_err(|e| format!("GetDesc1 failed: {e}"))?;
        let len = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        out.push(Adapter {
            index,
            adapter,
            name: String::from_utf16_lossy(&desc.Description[..len]),
            vendor: desc.VendorId,
            device: desc.DeviceId,
            software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
            vram_mb: desc.DedicatedVideoMemory as u64 / (1024 * 1024),
        });
    }
    Ok(out)
}

// --- The device ----------------------------------------------------------

pub struct Device {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub winrt: IDirect3DDevice,
}

pub fn create_device(adapter: &IDXGIAdapter1) -> Result<Device, String> {
    let adapter: IDXGIAdapter = adapter
        .cast()
        .map_err(|e| format!("the adapter is not an IDXGIAdapter: {e}"))?;
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    // SAFETY: the adapter is live; with an explicit adapter the driver type
    // must be UNKNOWN and the software module null. Both out-parameters are
    // live locals.
    unsafe {
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            // BGRA because WGC delivers it; VIDEO because the sink writer's
            // video processor and a hardware encoder both run on this device.
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|e| format!("D3D11CreateDevice failed: {e}"))?;
    let device = device.ok_or("D3D11CreateDevice returned no device")?;
    let context = context.ok_or("D3D11CreateDevice returned no context")?;

    // The immediate context is shared between this thread's copies and the
    // encoder's own threads, through the DXGI device manager. Without this
    // the two race on it, and the symptom is corruption rather than an error.
    let multithread: ID3D11Multithread = device
        .cast()
        .map_err(|e| format!("the device has no ID3D11Multithread: {e}"))?;
    // SAFETY: `multithread` is live; the call only sets a flag.
    let _ = unsafe { multithread.SetMultithreadProtected(true) };

    let dxgi: IDXGIDevice = device
        .cast()
        .map_err(|e| format!("the D3D device is not a DXGI device: {e}"))?;
    // SAFETY: `dxgi` is a live DXGI device.
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .map_err(|e| format!("CreateDirect3D11DeviceFromDXGIDevice failed: {e}"))?;
    let winrt = inspectable
        .cast()
        .map_err(|e| format!("the WinRT device is not an IDirect3DDevice: {e}"))?;
    Ok(Device {
        device,
        context,
        winrt,
    })
}

// --- Capture targets -----------------------------------------------------

pub struct Monitor {
    pub handle: HMONITOR,
    pub bounds: RECT,
}

/// # Safety
///
/// Only as the callback of `EnumDisplayMonitors` in [`monitors`], whose
/// `lparam` is a live `*mut Vec<Monitor>` for the duration of that call.
unsafe extern "system" fn collect_monitor(
    handle: HMONITOR,
    _dc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: the caller's contract: `lparam` points at a live Vec, and the
    // enumeration is synchronous so nothing else touches it meanwhile.
    let found = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `handle` came from the enumeration and `info` has its size set.
    if unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() {
        found.push(Monitor {
            handle,
            bounds: info.rcMonitor,
        });
    }
    TRUE
}

pub fn monitors() -> Vec<Monitor> {
    let mut found: Vec<Monitor> = Vec::new();
    // SAFETY: `collect_monitor` only touches the Vec behind `lparam`, which
    // outlives this synchronous call.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&raw mut found as isize),
        )
    };
    found
}

pub struct Window {
    pub handle: HWND,
    pub title: String,
    pub pid: u32,
}

fn describe_window(handle: HWND) -> Window {
    let mut buffer = [0u16; 512];
    // SAFETY: `buffer` is a live, writable slice; a stale handle yields 0.
    let len = unsafe { GetWindowTextW(handle, &mut buffer) }.max(0) as usize;
    let mut pid = 0u32;
    // SAFETY: a stale handle makes this return 0 rather than misbehave.
    unsafe { GetWindowThreadProcessId(handle, Some(&mut pid)) };
    Window {
        handle,
        title: String::from_utf16_lossy(&buffer[..len]),
        pid,
    }
}

/// # Safety
///
/// Only as the callback of `EnumWindows` in [`visible_windows`], whose
/// `lparam` is a live `*mut Vec<Window>` for the duration of that call.
unsafe extern "system" fn collect_window(handle: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: the caller's contract, as for `collect_monitor`.
    let found = unsafe { &mut *(lparam.0 as *mut Vec<Window>) };
    // SAFETY: `handle` came from the enumeration.
    if unsafe { IsWindowVisible(handle) }.as_bool() {
        let window = describe_window(handle);
        if !window.title.is_empty() {
            found.push(window);
        }
    }
    TRUE
}

pub fn visible_windows() -> Vec<Window> {
    let mut found: Vec<Window> = Vec::new();
    // SAFETY: `collect_window` only touches the Vec behind `lparam`, which
    // outlives this synchronous call.
    let _ = unsafe { EnumWindows(Some(collect_window), LPARAM(&raw mut found as isize)) };
    found
}

/// The game window by class, as `recorder/libobs/window.rs` finds it. The
/// title is locale-dependent; the class is not.
pub fn game_window() -> Option<Window> {
    // SAFETY: a static, NUL-terminated class name and a null title.
    let handle = unsafe { FindWindowW(w!("RiotWindowClass"), None) }.ok()?;
    Some(describe_window(handle))
}

pub struct Resolved {
    pub item: GraphicsCaptureItem,
    pub description: String,
}

pub fn capture_item(target: &Target) -> Result<Resolved, String> {
    // WGC's items are WinRT and the handles are Win32; the bridge is an
    // interop interface on the activation factory.
    let interop: IGraphicsCaptureItemInterop =
        windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|e| format!("no capture interop factory: {e} (needs Windows 10 1803+)"))?;

    let from_window = |window: Window, how: &str| -> Result<Resolved, String> {
        // SAFETY: `window.handle` came from a live enumeration; a window that
        // has closed since makes this fail with an error, not misbehave.
        let item = unsafe { interop.CreateForWindow::<GraphicsCaptureItem>(window.handle) }
            .map_err(|e| format!("CreateForWindow failed: {e}"))?;
        Ok(Resolved {
            item,
            description: format!("window {:?}, pid {} ({how})", window.title, window.pid),
        })
    };

    match target {
        Target::Game => {
            let window = game_window().ok_or(
                "no League game window (class RiotWindowClass). The game window exists only \
                 from loading screen to end of game; start one, or pick a target from --list",
            )?;
            from_window(window, "class RiotWindowClass")
        }
        Target::Window(needle) => {
            let lower = needle.to_lowercase();
            let window = visible_windows()
                .into_iter()
                .find(|w| w.title.to_lowercase().contains(&lower))
                .ok_or_else(|| {
                    format!("no visible window whose title contains {needle:?}; try --list")
                })?;
            from_window(window, "title match")
        }
        Target::Monitor(index) => {
            let found = monitors();
            let monitor = found
                .get(*index)
                .ok_or_else(|| format!("no monitor {index}; --list shows {}", found.len()))?;
            // SAFETY: the handle came from the enumeration just now.
            let item = unsafe { interop.CreateForMonitor::<GraphicsCaptureItem>(monitor.handle) }
                .map_err(|e| format!("CreateForMonitor failed: {e}"))?;
            let b = monitor.bounds;
            Ok(Resolved {
                item,
                description: format!("monitor {index}, {}x{}", b.right - b.left, b.bottom - b.top),
            })
        }
    }
}

// --- Slots ---------------------------------------------------------------

/// Counts a slot's samples back in: Media Foundation calls this when a
/// tracked sample's last reference goes, which is when the encoder has
/// finished reading the texture behind it.
#[implement(IMFAsyncCallback)]
struct Release(Arc<AtomicU32>);

impl IMFAsyncCallback_Impl for Release_Impl {
    fn GetParameters(&self, _flags: *mut u32, _queue: *mut u32) -> windows::core::Result<()> {
        // E_NOTIMPL is the documented "use the defaults".
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
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
    pub fn busy(&self) -> bool {
        self.in_flight.load(Ordering::Acquire) > 0
    }
}

pub fn texture_desc(width: u32, height: u32, staging: bool) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        // The video processor reads these as shader resources; render target
        // is what lets it treat them as a surface on some drivers.
        BindFlags: if staging {
            0
        } else {
            (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32
        },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    }
}

pub fn create_texture(
    device: &ID3D11Device,
    desc: &D3D11_TEXTURE2D_DESC,
) -> Result<ID3D11Texture2D, String> {
    let mut texture: Option<ID3D11Texture2D> = None;
    // SAFETY: `desc` is a live, fully initialised descriptor; no initial data.
    unsafe { device.CreateTexture2D(desc, None, Some(&mut texture)) }
        .map_err(|e| format!("CreateTexture2D failed: {e}"))?;
    texture.ok_or_else(|| "CreateTexture2D returned nothing".to_string())
}

pub fn create_slots(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    count: usize,
) -> Result<Vec<Slot>, String> {
    let desc = texture_desc(width, height, false);
    (0..count)
        .map(|_| {
            let in_flight = Arc::new(AtomicU32::new(0));
            Ok(Slot {
                texture: create_texture(device, &desc)?,
                callback: Release(in_flight.clone()).into(),
                in_flight,
            })
        })
        .collect()
}

/// Copy the top-left `width` x `height` of `source` into `slot`. A window
/// that has grown is cropped and one that has shrunk leaves the rest of the
/// slot stale, both of which the report counts rather than fixes.
pub fn copy_into(
    context: &ID3D11DeviceContext,
    slot: &Slot,
    source: &ID3D11Texture2D,
    width: u32,
    height: u32,
) {
    let region = D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: width,
        bottom: height,
        back: 1,
    };
    // SAFETY: both textures are live on the same device, share a format, and
    // the box lies within both (the caller clamps it to the smaller).
    unsafe { context.CopySubresourceRegion(&slot.texture, 0, 0, 0, 0, source, 0, Some(&region)) };
}

/// Mean brightness (0-255) of a sparse grid over `texture`, read back
/// through `staging`. Answers "did WGC deliver the game or a black
/// rectangle", which a frame count alone cannot: exclusive fullscreen and
/// some overlays give WGC black frames at full rate.
pub fn brightness(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    texture: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Option<f64> {
    // SAFETY: same device, same size and format.
    unsafe { context.CopyResource(staging, texture) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: `staging` is a CPU-readable staging texture; `mapped` is live.
    unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.ok()?;
    let mut sum = 0u64;
    let mut count = 0u64;
    let base = mapped.pData as *const u8;
    for gy in 0..18u32 {
        for gx in 0..32u32 {
            let x = (gx * 2 + 1) * width / 64;
            let y = (gy * 2 + 1) * height / 36;
            let offset = y as usize * mapped.RowPitch as usize + x as usize * 4;
            // SAFETY: the mapping is `RowPitch * height` bytes, and x < width,
            // y < height, so the four bytes at `offset` are inside it.
            let pixel = unsafe { std::slice::from_raw_parts(base.add(offset), 3) };
            sum += u64::from(pixel[0]) + u64::from(pixel[1]) + u64::from(pixel[2]);
            count += 3;
        }
    }
    // SAFETY: pairs the successful Map above.
    unsafe { context.Unmap(staging, 0) };
    Some(sum as f64 / count as f64)
}
