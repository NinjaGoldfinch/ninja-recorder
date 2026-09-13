//! Audio input device enumeration for the microphone picker.
//!
//! Done directly against the Windows Core Audio APIs rather than by asking
//! libobs, for one reason: the picker has to work when the capture backend
//! *failed* to initialize. `lib.rs`'s setup falls back to `FailedRecorder`
//! when libobs can't start, and a user in that state should still be able to
//! see and change their settings rather than face an empty dropdown with no
//! explanation.
//!
//! `IMMDevice::GetId` returns exactly the string OBS's `wasapi_input_capture`
//! wants in its `device_id` setting — OBS builds its own device list the same
//! way — so the ids handed to the frontend can be passed straight back down
//! into the capture backend without translation.

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

    unsafe fn enumerate() -> Result<Vec<AudioInputDevice>, String> {
        // Not `?`-ed: S_FALSE means "already initialized on this thread",
        // which is a success. Only a real failure should stop us, and
        // `CoUninitialize` must still pair with any success.
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        if hr.is_err() {
            return Err(format!("CoInitializeEx failed: {hr:?}"));
        }
        let result = enumerate_inner();
        CoUninitialize();
        result
    }

    unsafe fn enumerate_inner() -> Result<Vec<AudioInputDevice>, String> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("could not create the device enumerator: {e}"))?;

        // OBS resolves a `device_id` of "default" for *input* via
        // eCommunications, not eConsole. Matching that here is what makes the
        // picker's "Windows default" entry mean the same device the recorder
        // will actually open.
        let default_id = enumerator
            .GetDefaultAudioEndpoint(eCapture, eCommunications)
            .ok()
            .and_then(|device| device.GetId().ok())
            .and_then(|id| take_pwstr(id));

        let collection = enumerator
            .EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
            .map_err(|e| format!("could not enumerate audio inputs: {e}"))?;
        let count = collection
            .GetCount()
            .map_err(|e| format!("could not count audio inputs: {e}"))?;

        let mut devices = Vec::with_capacity(count as usize);
        for i in 0..count {
            // One unreadable endpoint shouldn't hide every other microphone.
            let Ok(device) = collection.Item(i) else {
                continue;
            };
            let Some(id) = device.GetId().ok().and_then(|id| take_pwstr(id)) else {
                continue;
            };
            let name = device
                .OpenPropertyStore(STGM_READ)
                .ok()
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName).ok())
                .and_then(|value| propvariant_string(value))
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
    unsafe fn take_pwstr(ptr: PWSTR) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let out = PCWSTR(ptr.0).to_string().ok();
        CoTaskMemFree(Some(ptr.0 as *const core::ffi::c_void));
        out
    }

    /// Reads a string out of a `PROPVARIANT` and releases it.
    ///
    /// The windows-rs `PROPVARIANT` is the raw FFI struct with no `Drop`, so
    /// it needs an explicit `PropVariantClear`. Going through
    /// `PropVariantToStringAlloc` rather than reading the union directly
    /// means a device whose name isn't stored as `VT_LPWSTR` still converts
    /// instead of coming back empty.
    unsafe fn propvariant_string(mut value: PROPVARIANT) -> Option<String> {
        let out = PropVariantToStringAlloc(&value)
            .ok()
            .and_then(|p| take_pwstr(p));
        let _ = PropVariantClear(&mut value);
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
