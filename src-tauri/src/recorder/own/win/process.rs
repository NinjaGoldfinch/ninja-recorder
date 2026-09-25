//! The process table, and the game window's owner: the Windows half of
//! `own/root.rs`, which makes the decision. Ported from `spikes/p0c-audio`.
//!
//! **Nothing here touches the game process beyond reading it.** A Toolhelp
//! snapshot lists processes without opening them, and the creation time is
//! read through `PROCESS_QUERY_LIMITED_INFORMATION`, the least a handle can
//! carry: no memory access, no injection (DEVELOPMENT.md §1.1). The spike
//! read the same fields from a Vanguard-protected game on the box.

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HWND};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use crate::recorder::own::root::Proc;

/// An owned Win32 handle, closed on drop.
pub struct OwnedHandle(pub HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle is owned by this value and closed once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Every process, with its parent and, where it can be read, its creation
/// time.
pub fn snapshot() -> Result<Vec<Proc>, String> {
    // SAFETY: no pointers in; the returned handle is owned below.
    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| format!("CreateToolhelp32Snapshot failed: {e}"))?;
    let handle = OwnedHandle(handle);

    let mut entry =
        PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut procs = Vec::new();
    // SAFETY: `entry` is live with `dwSize` set, which is what both calls
    // require, and the snapshot handle is live.
    let mut more = unsafe { Process32FirstW(handle.0, &mut entry) }.is_ok();
    while more {
        let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        procs.push(Proc {
            pid: entry.th32ProcessID,
            ppid: entry.th32ParentProcessID,
            exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
            created: creation_time(entry.th32ProcessID),
        });
        // SAFETY: as above.
        more = unsafe { Process32NextW(handle.0, &mut entry) }.is_ok();
    }
    Ok(procs)
}

fn creation_time(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    // SAFETY: plain call; the handle it returns is owned below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let handle = OwnedHandle(handle);
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the handle is live with query rights, and all four
    // out-parameters are live FILETIMEs.
    unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
        .ok()?;
    Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// The PID that owns `hwnd`, or `None` for a window that has gone.
pub fn window_owner(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: `pid` is a live out-parameter; a stale window makes this
    // return 0 rather than misbehave.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}
