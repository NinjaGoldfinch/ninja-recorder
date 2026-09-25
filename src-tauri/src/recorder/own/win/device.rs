//! The machine: its Windows build, its GPUs, the D3D11 device capture and
//! encoding share, and the clock both run on. Ported from
//! `spikes/p0c-video/src/win/video.rs` and `mod.rs`.

use std::sync::OnceLock;

use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice,
    IDXGIFactory1,
};
use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::core::{Interface, w};

use crate::recorder::own::clock::HNS_PER_SECOND;
use crate::recorder::own::select;

/// The Windows build number, e.g. 26200, or `None` if it cannot be read.
///
/// `RtlGetVersion`, because `GetVersionEx` lies to an executable with no
/// compatibility manifest.
pub fn windows_build() -> Option<u32> {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is live with its size field set, which is the whole of
    // RtlGetVersion's contract.
    unsafe { RtlGetVersion(&mut info) }.ok().ok()?;
    Some(info.dwBuildNumber)
}

/// The performance counter in 100 ns units: the timebase WGC's
/// `SystemRelativeTime` and WASAPI's packet positions are reported in, and
/// the one the video tick grid is laid on.
pub fn qpc_hns() -> i64 {
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: `f` is a live out-parameter; this cannot fail on XP or later.
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    });
    let mut count = 0i64;
    // SAFETY: `count` is a live out-parameter.
    let _ = unsafe { QueryPerformanceCounter(&mut count) };
    (i128::from(count) * i128::from(HNS_PER_SECOND) / i128::from(frequency)) as i64
}

/// Whether Media Foundation is installed, asked without calling into it.
///
/// **Checked before the first MF call, every time.** `mfplat.dll` and
/// `mfreadwrite.dll` are delay-loaded (`build.rs`), so a Windows N edition
/// without the Media Feature Pack can still start the app and record on
/// libobs. The price is that calling an MF function there would raise an
/// exception from the delay-load helper instead of returning an error, so
/// nothing may reach one until this has said yes.
pub fn media_foundation() -> Result<(), String> {
    for dll in [w!("mfplat.dll"), w!("mfreadwrite.dll")] {
        // SAFETY: a static, NUL-terminated name; System32 only, so nothing on
        // the search path can stand in for it. The module stays loaded for
        // the process's life, which is what the delay-load helper would do.
        if let Err(e) = unsafe { LoadLibraryExW(dll, None, LOAD_LIBRARY_SEARCH_SYSTEM32) } {
            // SAFETY: `dll` is one of the static names above.
            let name = unsafe { dll.to_string() }.unwrap_or_default();
            return Err(format!(
                "Media Foundation is not installed ({name}: {e}); a Windows N edition needs the \
                 Media Feature Pack"
            ));
        }
    }
    Ok(())
}

// --- Adapters ------------------------------------------------------------

/// One DXGI adapter, the COM object alongside the plain description
/// `select::rank` reads.
pub struct Adapter {
    pub adapter: IDXGIAdapter1,
    pub info: select::Adapter,
}

pub fn adapters() -> Result<Vec<Adapter>, String> {
    // SAFETY: no arguments; the factory is a COM object owned by the result.
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|e| format!("CreateDXGIFactory1 failed: {e}"))?;
    let mut out = Vec::new();
    for index in 0.. {
        // SAFETY: `factory` is live; an index past the end is
        // DXGI_ERROR_NOT_FOUND, which ends the loop.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        // SAFETY: `adapter` is live.
        let desc = unsafe { adapter.GetDesc1() }.map_err(|e| format!("GetDesc1 failed: {e}"))?;
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        out.push(Adapter {
            adapter,
            info: select::Adapter {
                name: String::from_utf16_lossy(&desc.Description[..len]),
                vendor: desc.VendorId,
                software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
            },
        });
    }
    Ok(out)
}

// --- The device ----------------------------------------------------------

/// The D3D11 device, its immediate context, and the WinRT wrapper WGC's
/// frame pool takes. One per warm session: the capture copies and the
/// encoder both run on it.
pub struct Device {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub winrt: IDirect3DDevice,
}

/// A device on `adapter`.
pub fn create_device(adapter: &IDXGIAdapter1) -> Result<Device, String> {
    let adapter: IDXGIAdapter =
        adapter.cast().map_err(|e| format!("the adapter is not an IDXGIAdapter: {e}"))?;
    create(
        Some(&adapter),
        D3D_DRIVER_TYPE_UNKNOWN,
        // BGRA because WGC delivers it; VIDEO because the sink writer's video
        // processor and a hardware encoder both run on this device.
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    )
}

/// A WARP (software rasteriser) device, for the CI test that has no GPU.
/// Video support is asked for first and dropped if WARP refuses it; the
/// software encoder does not need it.
#[cfg(test)]
pub fn create_warp_device() -> Result<Device, String> {
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
    create(
        None,
        D3D_DRIVER_TYPE_WARP,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    )
    .or_else(|_| create(None, D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_BGRA_SUPPORT))
}

fn create(
    adapter: Option<&IDXGIAdapter>,
    driver: D3D_DRIVER_TYPE,
    flags: D3D11_CREATE_DEVICE_FLAG,
) -> Result<Device, String> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    // SAFETY: the adapter, if any, is live; with an explicit adapter the
    // driver type is UNKNOWN, and the software module is null either way.
    // Both out-parameters are live locals.
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver,
            HMODULE::default(),
            flags,
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
    let multithread: ID3D11Multithread =
        device.cast().map_err(|e| format!("the device has no ID3D11Multithread: {e}"))?;
    // SAFETY: `multithread` is live; the call only sets a flag.
    let _ = unsafe { multithread.SetMultithreadProtected(true) };

    let dxgi: IDXGIDevice =
        device.cast().map_err(|e| format!("the D3D device is not a DXGI device: {e}"))?;
    // SAFETY: `dxgi` is a live DXGI device.
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .map_err(|e| format!("CreateDirect3D11DeviceFromDXGIDevice failed: {e}"))?;
    let winrt = inspectable
        .cast()
        .map_err(|e| format!("the WinRT device is not an IDirect3DDevice: {e}"))?;
    Ok(Device { device, context, winrt })
}
