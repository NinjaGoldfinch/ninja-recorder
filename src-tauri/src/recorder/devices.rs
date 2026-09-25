//! Audio input device enumeration for the microphone picker.
//!
//! Done directly against the Windows Core Audio APIs rather than by asking
//! libobs, for one reason: the picker has to work when the capture backend
//! *failed* to initialize. `lib.rs`'s setup falls back to `FailedRecorder`
//! when libobs can't start, and a user in that state should still be able to
//! see and change their settings rather than face an empty dropdown with no
//! explanation.
//!
//! The id handed to the frontend is `IMMDevice::GetId`'s endpoint ID string.
//! Microsoft documents it as the way to reopen the same endpoint "at a later
//! time or in a different process" through `IMMDeviceEnumerator::GetDevice`,
//! and as opaque, so nothing here parses it. That is exactly what the own
//! backend does with the stored id (`own/win/audio/endpoint.rs`), in the
//! capture worker, possibly days after the picker listed it. Both backends
//! take the string as it is, without translation.
//!
//! - <https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdevice-getid>
//! - <https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdeviceenumerator-getdevice>
//! - <https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdeviceenumerator-enumaudioendpoints>

use super::audio::AudioInputDevice;

/// Every active audio input, default first.
///
/// Errors are the caller's to surface: an empty list and a failure to
/// enumerate are different states, and the settings screen says so rather
/// than silently offering only "Windows default".
pub fn list_audio_inputs() -> Result<Vec<AudioInputDevice>, String> {
    imp::list_audio_inputs()
}

#[cfg(target_os = "windows")]
mod imp {
    use super::AudioInputDevice;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::StructuredStorage::{
        PropVariantClear, PropVariantToStringAlloc, PROPVARIANT,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED, STGM_READ,
    };

    pub fn list_audio_inputs() -> Result<Vec<AudioInputDevice>, String> {
        // Tauri's main thread is STA — WebView2 requires it — so initializing
        // MTA there returns RPC_E_CHANGED_MODE. Own a thread for the duration
        // instead of trying to share the app's apartment.
        std::thread::scope(|scope| {
            scope
                .spawn(|| unsafe { enumerate() })
                .join()
                .map_err(|_| "audio device enumeration thread panicked".to_string())?
        })
    }

    /// # Safety
    ///
    /// Must be called on a thread that is not already in a single-threaded
    /// apartment, and that thread must not be used for anything else COM
    /// until this returns — it initializes MTA and uninitializes it again.
    /// `list_audio_inputs` owns a scoped thread for exactly that reason.
    unsafe fn enumerate() -> Result<Vec<AudioInputDevice>, String> {
        // Not `?`-ed: S_FALSE means "already initialized on this thread",
        // which is a success. Only a real failure should stop us, and
        // `CoUninitialize` must still pair with any success.
        //
        // SAFETY: called on a thread this function owns for its whole
        // duration (the caller's contract above), so no other apartment
        // choice is in force and nothing else on the thread is holding a
        // COM pointer across the `CoUninitialize` below.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            return Err(format!("CoInitializeEx failed: {hr:?}"));
        }
        // SAFETY: COM is initialized on this thread and stays initialized
        // until the `CoUninitialize` below, which runs after every
        // interface pointer `enumerate_inner` created has been dropped —
        // it returns plain `String`s, holding nothing.
        let result = unsafe { enumerate_inner() };
        // SAFETY: pairs with the successful `CoInitializeEx` above, on the
        // same thread, with no live COM pointers outstanding.
        unsafe { CoUninitialize() };
        result
    }

    /// # Safety
    ///
    /// COM must be initialized on the calling thread, and must stay
    /// initialized until every value this returns has been dropped.
    unsafe fn enumerate_inner() -> Result<Vec<AudioInputDevice>, String> {
        // SAFETY: COM is initialized on this thread (the caller's
        // contract), and `MMDeviceEnumerator`/`IMMDeviceEnumerator` are a
        // matching CLSID/interface pair, so the returned pointer really is
        // the interface the binding claims.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|e| format!("could not create the device enumerator: {e}"))?;

        // "Windows default" is the default capture endpoint for the
        // communications role: `GetDefaultAudioEndpoint(eCapture,
        // eCommunications)`. Microsoft's page says a user can assign the
        // console, multimedia and communications roles to different devices,
        // and that an application managing voice streams asks for the
        // communications one; a microphone in a game VOD is voice. The own
        // backend resolves a microphone with no configured id the same way
        // (`own/win/audio/endpoint.rs`), and that match is what makes this
        // entry name the device the recorder will actually open.
        // <https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdeviceenumerator-getdefaultaudioendpoint>
        // <https://learn.microsoft.com/en-us/windows/win32/coreaudio/device-roles>
        //
        // SAFETY, all three: `enumerator` is a live interface pointer on a
        // thread with COM initialized, so is the `device` it returns, and
        // `GetId` hands over a `CoTaskMemAlloc`-ed string — which is exactly
        // what `take_pwstr` takes ownership of and frees.
        let default_id = unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eCommunications) }
            .ok()
            .and_then(|device| unsafe { device.GetId() }.ok())
            .and_then(|id| unsafe { take_pwstr(id) });

        // SAFETY: as above — a live interface pointer, COM initialized, and
        // neither call takes ownership of anything we hold.
        let collection = unsafe { enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) }
            .map_err(|e| format!("could not enumerate audio inputs: {e}"))?;
        let count = unsafe { collection.GetCount() }
            .map_err(|e| format!("could not count audio inputs: {e}"))?;

        let mut devices = Vec::with_capacity(count as usize);
        for i in 0..count {
            // One unreadable endpoint shouldn't hide every other microphone.
            //
            // SAFETY: `i` is below the count `collection` just reported, so
            // the index is in range; COM is initialized for the whole loop.
            let Ok(device) = (unsafe { collection.Item(i) }) else {
                continue;
            };
            // SAFETY: `device` is a live interface pointer, and `GetId`
            // transfers ownership of a `CoTaskMemAlloc`-ed string to
            // `take_pwstr`, which is what frees it.
            let Some(id) = (unsafe { device.GetId() })
                .ok()
                .and_then(|id| unsafe { take_pwstr(id) })
            else {
                continue;
            };
            // SAFETY, all three: `device` is live, so is the property store
            // it opens, and `GetValue` fills a `PROPVARIANT` we then own
            // outright and hand to `propvariant_string` — which is what
            // clears it, so it is read and released exactly once.
            let name = unsafe { device.OpenPropertyStore(STGM_READ) }
                .ok()
                .and_then(|store| unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok())
                .and_then(|value| unsafe { propvariant_string(value) })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Unknown input".to_string());

            let is_default = default_id.as_deref() == Some(id.as_str());
            devices.push(AudioInputDevice { id, name, is_default });
        }

        devices.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
        Ok(devices)
    }

    /// Copies a COM-allocated wide string out, then frees the original.
    ///
    /// `IMMDevice::GetId` and `PropVariantToStringAlloc` both allocate with
    /// `CoTaskMemAlloc` and hand over ownership, so skipping the free leaks
    /// once per device per refresh.
    /// # Safety
    ///
    /// `ptr` must be null, or a `CoTaskMemAlloc`-ed, null-terminated wide
    /// string whose ownership is being transferred to this function. It is
    /// freed here, so no other copy of the pointer may be used afterwards.
    unsafe fn take_pwstr(ptr: PWSTR) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: non-null by the check above, and null-terminated by the
        // caller's contract, so the read stops inside the allocation.
        let out = unsafe { PCWSTR(ptr.0).to_string() }.ok();
        // SAFETY: `ptr` was `CoTaskMemAlloc`-ed and ownership was
        // transferred to us, so this is the one and only free of it. The
        // string above is already copied out.
        unsafe { CoTaskMemFree(Some(ptr.0 as *const core::ffi::c_void)) };
        out
    }

    /// Reads a string out of a `PROPVARIANT` and releases it.
    ///
    /// windows-rs 0.62 gives `PROPVARIANT` a `Drop` that calls
    /// `PropVariantClear`, so the explicit clear below is belt and braces:
    /// it leaves the value `VT_EMPTY`, and the clear on drop then has nothing
    /// to free. (The p0c-audio spike is the case where that `Drop` bites: a
    /// `VT_BLOB` pointing at the stack, #7.) Going through
    /// `PropVariantToStringAlloc` rather than reading the union directly
    /// means a device whose name isn't stored as `VT_LPWSTR` still converts
    /// instead of coming back empty.
    /// # Safety
    ///
    /// `value` must be a fully initialized `PROPVARIANT` whose ownership is
    /// being transferred to this function; it is cleared here, so no other
    /// copy may be used or cleared afterwards.
    unsafe fn propvariant_string(mut value: PROPVARIANT) -> Option<String> {
        // SAFETY: `value` is initialized by the caller's contract, and the
        // string `PropVariantToStringAlloc` returns is `CoTaskMemAlloc`-ed
        // and ours, which is exactly what `take_pwstr` frees.
        let out = unsafe { PropVariantToStringAlloc(&value) }
            .ok()
            .and_then(|p| unsafe { take_pwstr(p) });
        // SAFETY: `value` is initialized and owned by us, and the string
        // above was copied out, not aliased. The clear on drop that follows
        // sees `VT_EMPTY` and frees nothing.
        let _ = unsafe { PropVariantClear(&mut value) };
        out
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::AudioInputDevice;

    /// No capture backend exists off Windows (`StubRecorder` records
    /// nothing), so there is nothing to enumerate. An empty list rather than
    /// an error: the settings screen then shows only "Windows default",
    /// which is the truth on a machine that can't record.
    pub fn list_audio_inputs() -> Result<Vec<AudioInputDevice>, String> {
        Ok(Vec::new())
    }
}
