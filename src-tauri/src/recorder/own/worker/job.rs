//! The job object that ties the capture worker's life to the daemon's.
//!
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` makes Windows terminate every process
//! in the job when the last handle to it closes. The daemon holds the only
//! handle, so however the daemon ends (a crash, Task Manager, the installer
//! killing it by name), the handle closes and the worker goes with it. A
//! worker is never left recording with nobody to stop it.
//!
//! One job per worker, closed after the worker has exited, so closing it
//! kills nothing that was still wanted.

use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::core::PCWSTR;

/// An anonymous job with kill-on-close set, holding one worker.
pub struct Job(HANDLE);

// SAFETY: a job handle is a kernel handle; any thread may use or close it.
unsafe impl Send for Job {}

impl Job {
    /// Creates the job and puts `child` in it.
    pub fn contain(child: &Child) -> Result<Job, String> {
        // SAFETY: no security attributes and no name: a fresh, unnamed job
        // whose only handle is the one returned.
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|e| format!("CreateJobObjectW: {e}"))?;
        let job = Job(handle);

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is a live JOBOBJECT_EXTENDED_LIMIT_INFORMATION for
        // the duration of the call, and the length passed is its size.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(|e| format!("SetInformationJobObject: {e}"))?;

        // SAFETY: `child` owns a live process handle for as long as it is
        // borrowed here, and `job.0` is the job created above.
        unsafe { AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle())) }
            .map_err(|e| format!("AssignProcessToJobObject: {e}"))?;
        Ok(job)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: this handle is owned by this value and closed only here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}
