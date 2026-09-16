//! Who is allowed to talk to the daemon. Implementation plan §4.2.
//!
//! The daemon will start a recording, delete a recording, and run whatever is
//! in its command table for anyone who can open its pipe. `reject_remote_clients`
//! already keeps that to this machine. This keeps it to this *user*.
//!
//! ## What a default pipe allows, and why that is not enough
//!
//! A named pipe created with no security attributes gets the creating process's
//! default DACL, which on a normal account grants the user and `SYSTEM` full
//! control. That is nearly right, and "nearly" is doing real work: the default
//! DACL comes from the token and can be widened by policy, a service account's
//! default differs, and none of it is written down anywhere a reader of this
//! code would find it. An explicit DACL says what is intended rather than
//! inheriting what happens to be configured.
//!
//! ## SDDL rather than hand-built ACLs
//!
//! `InitializeAcl` plus `AddAccessAllowedAce` plus `SetSecurityDescriptorDacl`
//! is four allocations and three chances to get a length wrong, in `unsafe`,
//! for a descriptor that is a one-line string in SDDL. The string is also the
//! thing a reader can check against the pipe's actual ACL with one PowerShell
//! command, which matters for something only verifiable on Windows.
//!
//! ## Failure degrades rather than refuses
//!
//! Every step here can fail in principle. If any does, the daemon falls back to
//! the default attributes and logs it, which is exactly the behaviour this file
//! replaced. A daemon that refused to start because it could not look up its
//! own SID would be a worse outcome than one running with the DACL Windows
//! would have given it anyway.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use crate::warn;

/// A security descriptor, and the `SECURITY_ATTRIBUTES` pointing at it.
///
/// **Not `Send`, and deliberately not made so.** It owns raw pointers, and the
/// only correct lifetime for it is the `CreateNamedPipe` call: Windows copies
/// the descriptor into the pipe object, so nothing needs it afterwards. A
/// caller that holds one across an `await` makes its future non-`Send`, which
/// is a compile error at the `tokio::spawn` rather than something to paper over
/// with an `unsafe impl` — and the fix, scoping it to the call, is what should
/// have been written anyway.
///
/// Owns the descriptor so it outlives the `CreateNamedPipe` call and is freed
/// afterwards: Windows copies the descriptor into the object, so it is needed
/// only for the duration of the call, and leaking it once per pipe instance
/// would be a slow leak in a process that runs for days.
pub struct PipeSecurity {
    attributes: SECURITY_ATTRIBUTES,
    descriptor: PSECURITY_DESCRIPTOR,
}

impl PipeSecurity {
    /// Builds a DACL granting this user, `SYSTEM` and the local administrators
    /// group, and nobody else.
    ///
    /// `None` means the descriptor could not be built and the caller should use
    /// the default attributes. The reason is logged here rather than returned,
    /// because there is exactly one thing a caller can do about it.
    pub fn current_user_only() -> Option<PipeSecurity> {
        let sid = match current_user_sid() {
            Ok(sid) => sid,
            Err(e) => {
                warn!("rpc", "cannot read this process's user SID, using the default pipe ACL: {e}");
                return None;
            }
        };

        // `D:P` is a protected DACL: no ACEs inherited from anywhere, so this
        // list is the whole list. `(A;;GA;;;<sid>)` allows generic-all to one
        // trustee. `SY` is Local System and `BA` the built-in administrators
        // group, both of which can take ownership of anything on the machine
        // regardless, so excluding them would buy nothing and would stop an
        // administrator diagnosing a stuck daemon.
        //
        // Deliberately no `WD` (Everyone) and no `AU` (Authenticated Users):
        // another account signed in to the same machine is exactly who this
        // keeps out.
        let sddl = format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)(A;;GA;;;BA)\0");
        let wide: Vec<u16> = sddl.encode_utf16().collect();

        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the
        // call, and `descriptor` is a valid writable `PSECURITY_DESCRIPTOR`.
        // On success the callee allocates with `LocalAlloc`, which `Drop`
        // releases with the matching `LocalFree`.
        let built = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        };
        if let Err(e) = built {
            warn!("rpc", "cannot build the pipe's security descriptor, using the default: {e}");
            return None;
        }

        Some(PipeSecurity {
            attributes: SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                // The pipe handle must not be inherited by anything this
                // process spawns. The daemon spawns ffmpeg and the libobs
                // worker, and neither has any business holding the endpoint.
                bInheritHandle: false.into(),
            },
            descriptor,
        })
    }

    /// The pointer `ServerOptions::create_with_security_attributes_raw` wants.
    ///
    /// Takes `&mut self` although it does not mutate: the Win32 signature is
    /// `LPSECURITY_ATTRIBUTES`, a mutable pointer, and handing out a `*mut`
    /// derived from a shared reference is the thing that makes it unsound.
    pub fn as_ptr(&mut self) -> *mut core::ffi::c_void {
        std::ptr::from_mut(&mut self.attributes).cast()
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if self.descriptor.0.is_null() {
            return;
        }
        // SAFETY: `descriptor` came from
        // `ConvertStringSecurityDescriptorToSecurityDescriptorW`, which
        // allocates with `LocalAlloc`, and this is the only place it is freed.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.descriptor.0)));
        }
    }
}

/// This process's user SID, as the string SDDL wants.
fn current_user_sid() -> Result<String, String> {
    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no
    // closing, and `token` is a valid writable `HANDLE`.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| format!("cannot open this process's token: {e}"))?;

    let sid = read_token_user_sid(token);

    // Closed on both paths. This runs once per pipe instance, and a daemon
    // that accepts clients for a week would otherwise leak a handle per
    // connection.
    // SAFETY: `token` came from `OpenProcessToken` above and is not used again.
    unsafe {
        let _ = CloseHandle(token);
    }

    sid
}

/// The `TOKEN_USER` half, split out so the caller can close the token on every
/// path rather than on each `?`.
fn read_token_user_sid(token: HANDLE) -> Result<String, String> {
    // Two calls, because the structure is variable-length: the first asks how
    // big, the second fills it in. The first is expected to fail with
    // `ERROR_INSUFFICIENT_BUFFER`, which is why its result is discarded rather
    // than checked.
    let mut needed = 0u32;
    // SAFETY: a null buffer with length 0 is the documented way to ask for the
    // size; `needed` is a valid writable `u32`.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return Err("the token reported a zero-length TOKEN_USER".to_string());
    }

    // **A `u64` buffer, not a `u8` one.** `TOKEN_USER` holds a pointer and so
    // needs 8-byte alignment; `Vec<u8>` guarantees only 1, and reading a
    // `TOKEN_USER` out of it would be unaligned even where it happens to work.
    let mut buffer = vec![0u64; needed.div_ceil(8) as usize];
    // SAFETY: `buffer` is at least `needed` bytes, writable, and aligned for
    // `TOKEN_USER`, which is what the sizing call above asked for.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .map_err(|e| format!("cannot read the token's user: {e}"))?;

    // SAFETY: on success the buffer holds a `TOKEN_USER` whose `Sid` points
    // into that same buffer, so both are valid for as long as `buffer` is.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };

    let mut text = windows::core::PWSTR::null();
    // SAFETY: `sid` is valid as above, and `text` is a valid writable `PWSTR`.
    // On success the callee allocates with `LocalAlloc`; it is freed below.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }
        .map_err(|e| format!("cannot render the SID as a string: {e}"))?;

    // SAFETY: `text` is a NUL-terminated UTF-16 string the call just wrote.
    let owned = unsafe { text.to_string() };
    // SAFETY: `text` came from `ConvertSidToStringSidW`, which allocates with
    // `LocalAlloc`, and this is the only place it is freed. Done before the
    // decoding result is unwrapped so a failure cannot leak it.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(text.as_ptr().cast())));
    }

    owned.map_err(|e| format!("the SID is not valid UTF-16: {e}"))
}
