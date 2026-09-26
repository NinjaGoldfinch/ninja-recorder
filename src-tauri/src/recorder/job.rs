//! The job object that ties a capture worker's life to the daemon's, for
//! both Windows backends.
//!
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` makes Windows terminate every process
//! in the job when the last handle to it closes. The daemon holds the only
//! handle, so however the daemon ends (a crash, Task Manager, the installer
//! killing it by name), the handle closes and the worker goes with it. A
//! worker is never left recording with nobody to stop it, and never left
//! writing a file the next daemon's startup recovery is about to repair
//! (#307).
//!
//! One job per worker, closed after the worker has exited, so closing it
//! kills nothing that was still wanted.
//!
//! Two ways in, because the two backends spawn their workers differently:
//!
//! - The own backend spawns `--capture-worker` itself, so it has the
//!   `Child` and uses [`Job::contain`].
//! - The libobs backend's worker, `extprocess_recorder.exe`, is spawned
//!   inside the `libobs-recorder` fork (`ipc-link`), which keeps the `Child`
//!   private. The libobs backend lists the daemon's children of that name
//!   before and after bringing the fork up, [`new_children`] picks the one
//!   that appeared, and [`Job::assign_pid`] puts it in the job.
//!
//! The Win32 half is Windows-only; the choice of which PIDs to assign is
//! pure, so it is tested everywhere.

/// One process from a process-table snapshot, reduced to what
/// [`new_children`] needs.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildProc {
    pub pid: u32,
    /// The parent PID recorded when the process was created.
    pub ppid: u32,
    /// The executable's file name, as Toolhelp reports it.
    pub exe: String,
}

/// The PIDs in `after` that are children of `parent`, run `exe` (compared
/// case-insensitively, as Windows compares file names), and were not already
/// in `before`.
///
/// `before` is what makes this safe to call on a table with a stale worker in
/// it: a worker from a previous bring-up that has not quite exited yet is in
/// both snapshots, and belongs to the job that is already closing on it. The
/// parent PID is only a number, and a process whose own parent died could in
/// principle carry one that has since been reused by the daemon; the name and
/// the before/after difference together are what rule that out.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn new_children(before: &[u32], after: &[ChildProc], parent: u32, exe: &str) -> Vec<u32> {
    after
        .iter()
        .filter(|p| p.ppid == parent && p.exe.eq_ignore_ascii_case(exe))
        .map(|p| p.pid)
        .filter(|pid| !before.contains(pid))
        .collect()
}

#[cfg(target_os = "windows")]
mod win {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};
    use windows::core::PCWSTR;

    use super::ChildProc;

    /// An owned Win32 handle, closed on drop.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle is owned by this value and closed only here.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// An anonymous job with kill-on-close set.
    pub struct Job(Owned);

    // SAFETY: a job handle is a kernel handle; any thread may use or close it.
    unsafe impl Send for Job {}

    impl Job {
        /// An empty job that kills whatever is in it when it is dropped.
        pub fn new() -> Result<Job, String> {
            // SAFETY: no security attributes and no name: a fresh, unnamed
            // job whose only handle is the one returned.
            let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
                .map_err(|e| format!("CreateJobObjectW: {e}"))?;
            let job = Job(Owned(handle));

            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` is a live JOBOBJECT_EXTENDED_LIMIT_INFORMATION
            // for the duration of the call, and the length passed is its size.
            unsafe {
                SetInformationJobObject(
                    job.0.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            }
            .map_err(|e| format!("SetInformationJobObject: {e}"))?;
            Ok(job)
        }

        /// Creates a job and puts `child` in it.
        pub fn contain(child: &Child) -> Result<Job, String> {
            let job = Job::new()?;
            // SAFETY: `child` owns a live process handle for as long as it is
            // borrowed here, and `job.0` is the job created above.
            unsafe { AssignProcessToJobObject(job.0.0, HANDLE(child.as_raw_handle())) }
                .map_err(|e| format!("AssignProcessToJobObject: {e}"))?;
            Ok(job)
        }

        /// Puts the process `pid` in this job. Needs no more access to it
        /// than assignment does: `PROCESS_SET_QUOTA | PROCESS_TERMINATE`.
        pub fn assign_pid(&self, pid: u32) -> Result<(), String> {
            // SAFETY: plain call; the handle it returns is owned below.
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }
                .map_err(|e| format!("OpenProcess({pid}): {e}"))?;
            let process = Owned(process);
            // SAFETY: both handles are live and owned for the whole call.
            unsafe { AssignProcessToJobObject(self.0.0, process.0) }
                .map_err(|e| format!("AssignProcessToJobObject({pid}): {e}"))
        }
    }

    /// Every process in the table, as Toolhelp lists it. Reads the table and
    /// opens no process.
    pub fn processes() -> Result<Vec<ChildProc>, String> {
        // SAFETY: no pointers in; the returned handle is owned below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|e| format!("CreateToolhelp32Snapshot: {e}"))?;
        let snapshot = Owned(snapshot);

        let mut entry =
            PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut procs = Vec::new();
        // SAFETY: `entry` is live with `dwSize` set, which is what both calls
        // require, and the snapshot handle is live.
        let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok();
        while more {
            let len =
                entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            procs.push(ChildProc {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
            });
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot.0, &mut entry) }.is_ok();
        }
        Ok(procs)
    }
}

#[cfg(target_os = "windows")]
pub use win::{Job, processes};

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, ppid: u32, exe: &str) -> ChildProc {
        ChildProc { pid, ppid, exe: exe.to_string() }
    }

    const WORKER: &str = "extprocess_recorder.exe";

    #[test]
    fn the_new_worker_is_picked() {
        let after = [proc(4, 1, "System"), proc(100, 50, WORKER), proc(101, 50, "ffmpeg.exe")];
        assert_eq!(new_children(&[], &after, 50, WORKER), [100]);
    }

    #[test]
    fn a_worker_that_was_already_there_is_not() {
        let after = [proc(90, 50, WORKER), proc(100, 50, WORKER)];
        assert_eq!(new_children(&[90], &after, 50, WORKER), [100]);
    }

    #[test]
    fn someone_elses_worker_is_not() {
        // Another daemon's, or one whose parent PID only matches by reuse
        // and which was there before the spawn.
        let after = [proc(100, 77, WORKER), proc(101, 50, WORKER)];
        assert_eq!(new_children(&[101], &after, 50, WORKER), Vec::<u32>::new());
    }

    #[test]
    fn the_name_is_compared_as_windows_does() {
        let after = [proc(100, 50, "ExtProcess_Recorder.EXE")];
        assert_eq!(new_children(&[], &after, 50, WORKER), [100]);
    }

    #[test]
    fn no_worker_is_no_pids() {
        let after = [proc(100, 50, "notepad.exe")];
        assert!(new_children(&[], &after, 50, WORKER).is_empty());
    }
}
