//! **P0c stage 1 (WS1.3, #7): can process loopback isolate game audio?**
//!
//! **Not a licence trade, though #7 and #67 were both written as though it
//! were.** The premise was that failing here means keeping libobs to keep
//! per-application audio. It does not: the fork captures per-app audio with
//! `wasapi_process_output_capture`, which is OBS's process-loopback source,
//! which is the same `ActivateAudioInterfaceAsync` +
//! `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK` this file calls. There is no
//! fallback inside it either. So if this fails, it fails for both backends and
//! keeping libobs saves nothing. #67 has the evidence.
//!
//! What that leaves is a question about Windows rather than about our code:
//! does process loopback work against a Vanguard-protected process? This spike
//! answers it in isolation, away from a capture pipeline that could be blamed
//! for the result.
//!
//! ## The root PID, which is the other half of the exit criterion
//!
//! #7 asks for the root PID to be "documented", and the question behind that
//! is the design document's §9 first open question: **which process is the
//! root of the tree process loopback captures?** Audio comes from
//! `League of Legends.exe`, not from the `LeagueClient*.exe` processes the LCU
//! integration tracks, and getting the root wrong produces silence rather than
//! an error. The plan's answer is the game process, found the way
//! `recorder/libobs/window.rs` finds the game window, then
//! `GetWindowThreadProcessId`.
//!
//! So before capturing, the spike prints the process table it is working
//! from: the root it resolved, where that PID came from (process name, the
//! game window's owner, or `--pid`), whether the two agree, the root's
//! ancestors and descendants, and every Discord, League client and Riot client
//! process with its relation to the root. "Discord is outside the tree" is the
//! structural half of "Discord absent"; the WAV is the other half.
//!
//! ## Running it
//!
//! `spikes/p0c-audio/README.md` is the procedure for #7. In short:
//!
//! ```text
//! cargo run --release -- --list                      # resolve and report, no capture
//! cargo run --release --                             # include the game's tree, 60 s
//! cargo run --release -- --mode exclude              # everything *but* the game's tree
//! cargo run --release -- --pid 1234 --seconds 30 --out game.wav
//! ```
//!
//! ## What it is deliberately not
//!
//! Not a recorder. There is no encoder, no container, no device switching and
//! no error recovery: it writes 32-bit float PCM into a RIFF file and stops.
//! Everything it leaves out is something `recorder/own/` will have to do, and
//! doing any of it here would make a failure ambiguous between "loopback does
//! not isolate" and "the spike is wrong", which is the one distinction the
//! whole exercise needs to keep clean.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!(
        "p0c-audio captures WASAPI process loopback, which exists only on Windows.\n\
         It is checked on other platforms (cargo check --target x86_64-pc-windows-msvc)\n\
         and run on the box."
    );
    std::process::exit(2);
}

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    match windows_impl::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("p0c-audio: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::fs::File;
    use std::mem::ManuallyDrop;
    use std::io::{BufWriter, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
        AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
        ActivateAudioInterfaceAsync, DEVICE_STATE_ACTIVE, IActivateAudioInterfaceAsyncOperation,
        IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
        IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        PROCESS_LOOPBACK_MODE, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        WAVEFORMATEX, eRender,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    use windows::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION, SetEvent, WaitForSingleObject,
    };
    use windows::Win32::System::Variant::VT_BLOB;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
    use windows::core::{Interface, PCWSTR, Ref, Result as WinResult, implement, w};

    /// `WAVE_FORMAT_IEEE_FLOAT`, spelled out rather than imported: windows-rs
    /// puts it behind `Win32_Media_KernelStreaming`, and pulling in a whole
    /// feature for one documented constant buys nothing.
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

    /// The format we ask for.
    ///
    /// **Process loopback gives no `GetMixFormat`**, which is the first
    /// surprise in this API: with a real endpoint you ask the device what it
    /// is doing and match it, and here there is no device to ask. The format
    /// below is what we assert, and the engine converts into it. 48 kHz
    /// stereo float is what the mix graph is natively doing on every machine
    /// this will ever run on, so it is the cheapest thing to ask for.
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u16 = 2;
    const BITS: u16 = 32;

    /// How long the activation may take before we call it hung.
    const ACTIVATION_TIMEOUT_MS: u32 = 5_000;

    /// How long one wait on the capture event lasts. Short, because a timeout
    /// is **not** an error here: a process-loopback stream is not guaranteed
    /// to be fed while its target is silent, so a quiet stretch of the game
    /// would otherwise end the run. It only bounds how late the per-second
    /// line is printed.
    const WAIT_SLICE_MS: u32 = 200;

    /// Below this a second counts as silent. -60 dBFS is well under anything
    /// a game or a voice call plays at, and well over float rounding noise.
    const SIGNAL_FLOOR_DBFS: f64 = -60.0;

    /// The game window, as `recorder/libobs/window.rs` names it. The title is
    /// locale-dependent, which is why a class-only lookup is tried as well.
    const GAME_WINDOW_CLASS: PCWSTR = w!("RiotWindowClass");
    const GAME_WINDOW_TITLE: PCWSTR = w!("League of Legends (TM) Client");
    const GAME_PROCESS: &str = "League of Legends.exe";

    /// Upper bound on `--seconds`. The RIFF sizes are `u32`, which holds about
    /// three hours of this format; nothing about the spike needs more than a
    /// few minutes.
    const MAX_SECONDS: u64 = 3_600;

    // --- Arguments -------------------------------------------------------

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        /// The target and its descendants, and nothing else. The #7 run.
        Include,
        /// Everything **except** the target and its descendants. The control:
        /// Discord should be in it and the game should not, which proves
        /// Discord was audible while the include run was not hearing it.
        Exclude,
    }

    impl Mode {
        fn name(self) -> &'static str {
            match self {
                Mode::Include => "include",
                Mode::Exclude => "exclude",
            }
        }

        fn api(self) -> PROCESS_LOOPBACK_MODE {
            match self {
                Mode::Include => PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                Mode::Exclude => PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
            }
        }
    }

    enum Target {
        Pid(u32),
        Name(String),
    }

    struct Args {
        target: Target,
        mode: Mode,
        seconds: u64,
        out: Option<PathBuf>,
        /// Report the environment and the process tree, and capture nothing.
        list_only: bool,
    }

    fn parse_args() -> Result<Args, String> {
        let mut target = None;
        let mut mode = Mode::Include;
        let mut seconds = 60u64;
        let mut out = None;
        let mut list_only = false;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--list" => list_only = true,
                "--pid" => {
                    let value = args.next().ok_or("--pid needs a process id")?;
                    let pid = value
                        .parse::<u32>()
                        .map_err(|_| format!("--pid wants a number, got {value:?}"))?;
                    target = Some(Target::Pid(pid));
                }
                "--name" => {
                    let value = args.next().ok_or("--name needs a process name")?;
                    target = Some(Target::Name(value));
                }
                "--mode" => {
                    let value = args.next().ok_or("--mode needs include or exclude")?;
                    mode = match value.as_str() {
                        "include" => Mode::Include,
                        "exclude" => Mode::Exclude,
                        _ => return Err(format!("--mode wants include or exclude, got {value:?}")),
                    };
                }
                "--seconds" => {
                    let value = args.next().ok_or("--seconds needs a number")?;
                    seconds = value
                        .parse::<u64>()
                        .ok()
                        .filter(|s| (1..=MAX_SECONDS).contains(s))
                        .ok_or_else(|| {
                            format!(
                                "--seconds wants a number from 1 to {MAX_SECONDS}, got {value:?}"
                            )
                        })?;
                }
                "--out" => {
                    out = Some(PathBuf::from(args.next().ok_or("--out needs a path")?));
                }
                "--help" | "-h" => return Err(usage()),
                other => return Err(format!("unknown argument {other:?}\n\n{}", usage())),
            }
        }

        Ok(Args {
            target: target.unwrap_or_else(|| Target::Name(GAME_PROCESS.to_string())),
            mode,
            seconds,
            out,
            list_only,
        })
    }

    fn usage() -> String {
        format!(
            "p0c-audio - WS1.3 (#7), the process-loopback spike\n\
             \n\
             \x20 --name <exe>       the root process, by image name (default {GAME_PROCESS:?})\n\
             \x20 --pid <n>          the root process, by id\n\
             \x20 --mode <m>         include (default): the root's tree and nothing else\n\
             \x20                    exclude: everything except the root's tree\n\
             \x20 --seconds <n>      how long for (default 60)\n\
             \x20 --out <path>       where the WAV goes (default p0c-audio-<mode>-<pid>.wav)\n\
             \x20 --list             report the environment and the process tree; capture nothing\n\
             \n\
             Run it with the game and Discord both making noise: the exit criterion is\n\
             that one of them is in the include file and the other is not."
        )
    }

    // --- Entry -----------------------------------------------------------

    pub fn run() -> Result<(), String> {
        let args = parse_args()?;

        // MTA for the duration. `recorder/devices.rs` owns a thread for the
        // same reason: an STA thread that tried this would get
        // RPC_E_CHANGED_MODE. This binary has no UI, so its main thread is
        // free to be MTA.
        // SAFETY: called once, on this thread, before any COM use, and paired
        // with the CoUninitialize below.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| format!("CoInitializeEx failed: {e}"))?;
        let result = run_initialized(&args);
        // SAFETY: pairs the successful CoInitializeEx above. Every COM object
        // `run_initialized` made has been dropped by the time it returns.
        unsafe { CoUninitialize() };
        result
    }

    fn run_initialized(args: &Args) -> Result<(), String> {
        println!("== p0c-audio (WS1.3, #7) ==");
        print_environment();

        let processes = snapshot()?;
        let root = resolve_root(&args.target, &processes)?;
        print_tree_report(&root, &processes);

        if args.list_only {
            return Ok(());
        }

        let out = args.out.clone().unwrap_or_else(|| {
            PathBuf::from(format!("p0c-audio-{}-{}.wav", args.mode.name(), root.pid))
        });
        capture(root.pid, args.mode, args.seconds, &out)
    }

    // --- Environment -----------------------------------------------------

    /// The two facts about the machine a result depends on: whether this
    /// Windows has process loopback at all, and whether we ran elevated. An
    /// elevated run that works says less than an unelevated one, because the
    /// shipping daemon is not elevated.
    fn print_environment() {
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` is a live OSVERSIONINFOW with its size field set,
        // which is the whole of RtlGetVersion's contract.
        let status = unsafe { RtlGetVersion(&mut info) };
        if status.is_ok() {
            println!(
                "windows        {}.{} build {}",
                info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
            );
            if info.dwBuildNumber < 19_041 {
                println!(
                    "               WARNING: older than 2004 (19041). Process loopback is not \
                     expected to exist here,\n               \
                     so an activation failure below is about the OS, not League."
                );
            }
        } else {
            println!("windows        RtlGetVersion failed ({status:?})");
        }

        let elevated = match is_elevated() {
            Some(true) => "yes (the daemon will not be: say so when reporting)",
            Some(false) => "no",
            None => "unknown",
        };
        println!("elevated       {elevated}");
        println!("render devices {}", render_endpoint_count());
        println!();
    }

    fn is_elevated() -> Option<bool> {
        let mut token = HANDLE::default();
        // SAFETY: GetCurrentProcess returns a pseudo-handle and has no
        // preconditions.
        let me = unsafe { GetCurrentProcess() };
        // SAFETY: `me` is valid for the life of the process, and `token` is a
        // live out-parameter; the handle it receives is owned below.
        unsafe { OpenProcessToken(me, TOKEN_QUERY, &mut token) }.ok()?;
        let token = OwnedHandle(token);

        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        // SAFETY: the buffer is a live TOKEN_ELEVATION and the length passed
        // is its size, which is what TokenElevation writes.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenElevation,
                Some((&raw mut elevation).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        }
        .ok()?;
        Some(elevation.TokenIsElevated != 0)
    }

    /// Not part of the exit criterion; it is here because the first question
    /// on a machine where nothing is captured is always "is anything playing
    /// at all". Process loopback does not target one of these: it targets a
    /// PID, and the endpoint it mixes through is chosen for us.
    fn render_endpoint_count() -> String {
        // SAFETY: COM is initialised on this thread (MTA) by `run`.
        let enumerator: IMMDeviceEnumerator =
            match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
                Ok(e) => e,
                Err(e) => return format!("unknown (device enumerator: {e})"),
            };
        // SAFETY: `enumerator` is a live COM object.
        let collection =
            match unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) } {
                Ok(c) => c,
                Err(e) => return format!("unknown (EnumAudioEndpoints: {e})"),
            };
        // SAFETY: `collection` is a live COM object.
        match unsafe { collection.GetCount() } {
            Ok(0) => "0 active - nothing can play, so nothing can be captured".to_string(),
            Ok(n) => format!("{n} active"),
            Err(e) => format!("unknown (GetCount: {e})"),
        }
    }

    // --- The process table -----------------------------------------------

    struct Proc {
        pid: u32,
        ppid: u32,
        exe: String,
        /// Creation time as a FILETIME tick count, where the process could be
        /// opened for it. It is what tells a live parent from a PID that was
        /// reused after the real parent exited.
        created: Option<u64>,
    }

    /// An owned Win32 handle, closed on drop.
    ///
    /// Several are live at once and one of them is handed to a COM object
    /// that outlives the stack frame that made it, which is exactly the shape
    /// that leaks a handle per run when the closing is written by hand at each
    /// early return.
    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: the handle is owned by this value and closed once.
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
    }

    fn new_event() -> Result<OwnedHandle, String> {
        // SAFETY: no security attributes and no name; an auto-reset event,
        // initially unsignalled. The handle is owned by the returned value.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(|e| format!("CreateEventW failed: {e}"))?;
        Ok(OwnedHandle(handle))
    }

    fn snapshot() -> Result<Vec<Proc>, String> {
        // SAFETY: no pointers in; the returned handle is owned below.
        let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|e| format!("CreateToolhelp32Snapshot failed: {e}"))?;
        let handle = OwnedHandle(handle);

        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut processes = Vec::new();
        // SAFETY: `entry` is a live PROCESSENTRY32W with `dwSize` set, which
        // is what both calls require, and the snapshot handle is live.
        let mut more = unsafe { Process32FirstW(handle.0, &mut entry) }.is_ok();
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            processes.push(Proc {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                exe: String::from_utf16_lossy(&entry.szExeFile[..len]),
                created: creation_time(entry.th32ProcessID),
            });
            // SAFETY: as above.
            more = unsafe { Process32NextW(handle.0, &mut entry) }.is_ok();
        }
        Ok(processes)
    }

    fn creation_time(pid: u32) -> Option<u64> {
        if pid == 0 {
            return None;
        }
        // SAFETY: plain call; the handle it returns is owned below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let handle = OwnedHandle(handle);
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: the handle is live with query rights and all four
        // out-parameters are live FILETIMEs.
        unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
            .ok()?;
        Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    fn find(processes: &[Proc], pid: u32) -> Option<&Proc> {
        processes.iter().find(|p| p.pid == pid)
    }

    /// The live parent of `child`, or `None` if its recorded parent exited.
    ///
    /// A PROCESSENTRY32's parent id is only a number recorded at creation, and
    /// Windows reuses numbers: a parent that exited leaves a PID that may now
    /// belong to something unrelated. A real parent was created before its
    /// child, so a "parent" created later is a reused number, not a parent.
    fn parent_of<'a>(processes: &'a [Proc], child: &Proc) -> Option<&'a Proc> {
        if child.ppid == 0 || child.ppid == child.pid {
            return None;
        }
        let parent = find(processes, child.ppid)?;
        match (parent.created, child.created) {
            (Some(p), Some(c)) if p > c => None,
            _ => Some(parent),
        }
    }

    fn ancestors<'a>(processes: &'a [Proc], of: &'a Proc) -> Vec<&'a Proc> {
        let mut chain = Vec::new();
        let mut current = of;
        // Bounded, because a snapshot is not atomic and a cycle is possible.
        while chain.len() < 64 {
            match parent_of(processes, current) {
                Some(parent) => {
                    chain.push(parent);
                    current = parent;
                }
                None => break,
            }
        }
        chain
    }

    fn is_in_tree(processes: &[Proc], root: u32, candidate: &Proc) -> bool {
        candidate.pid == root
            || ancestors(processes, candidate)
                .iter()
                .any(|p| p.pid == root)
    }

    struct Root {
        pid: u32,
        how: String,
    }

    fn resolve_root(target: &Target, processes: &[Proc]) -> Result<Root, String> {
        let window = game_window_owner();
        match target {
            Target::Pid(pid) => {
                if find(processes, *pid).is_none() {
                    return Err(format!("no process has PID {pid}."));
                }
                Ok(Root {
                    pid: *pid,
                    how: format!("--pid {pid}{}", window_agreement(window, *pid)),
                })
            }
            Target::Name(name) => {
                let matches: Vec<&Proc> = processes
                    .iter()
                    .filter(|p| p.exe.eq_ignore_ascii_case(name))
                    .collect();
                match matches.as_slice() {
                    [] => Err(format!(
                        "no process named {name:?} is running.\n\
                         For #7 the game has to be *in a game* (Practice Tool is fine), not at \
                         the client:\n`League of Legends.exe` only exists between loading \
                         screen and end of game.{}",
                        match window {
                            Some((pid, _)) => format!(
                                "\nThe game window is owned by PID {pid} ({}); pass --pid {pid} \
                                 if that is the game.",
                                find(processes, pid).map_or("?", |p| p.exe.as_str())
                            ),
                            None => String::new(),
                        }
                    )),
                    [only] => Ok(Root {
                        pid: only.pid,
                        how: format!("--name {name:?}{}", window_agreement(window, only.pid)),
                    }),
                    many => Err(format!(
                        "{} processes are named {name:?}: {}.\nPass --pid; which one is the \
                         root is exactly what #7 asks to be written down.",
                        many.len(),
                        many.iter()
                            .map(|p| p.pid.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
        }
    }

    /// The plan's way of finding the root: the game window, then its owner.
    /// Returns the owning PID and which lookup found it.
    fn game_window_owner() -> Option<(u32, &'static str)> {
        let lookups: [(PCWSTR, &'static str); 2] = [
            (
                GAME_WINDOW_TITLE,
                "class + title, as recorder/libobs/window.rs",
            ),
            (PCWSTR::null(), "class only"),
        ];
        for (title, how) in lookups {
            // SAFETY: both strings are static and NUL-terminated (`w!`), or null.
            let Ok(hwnd) = (unsafe { FindWindowW(GAME_WINDOW_CLASS, title) }) else {
                continue;
            };
            let mut pid = 0u32;
            // SAFETY: `hwnd` came from FindWindowW; a window that has since
            // closed makes this return 0 rather than misbehave.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid != 0 {
                return Some((pid, how));
            }
        }
        None
    }

    fn window_agreement(window: Option<(u32, &'static str)>, root: u32) -> String {
        match window {
            Some((pid, how)) if pid == root => {
                format!("; the game window ({how}) is owned by the same PID")
            }
            Some((pid, how)) => {
                format!("; WARNING: the game window ({how}) is owned by PID {pid}, not this one")
            }
            None => "; no game window found (RiotWindowClass)".to_string(),
        }
    }

    /// Everything the root-PID question needs, printed before the capture so
    /// it is in the output whatever the capture does.
    fn print_tree_report(root: &Root, processes: &[Proc]) {
        let describe = |p: &Proc| format!("{:>6}  {}", p.pid, p.exe);
        let Some(root_proc) = find(processes, root.pid) else {
            return;
        };

        println!("root           {}", describe(root_proc));
        println!("resolved by    {}", root.how);

        let chain = ancestors(processes, root_proc);
        println!("ancestors      (nearest first; not captured in include mode)");
        if chain.is_empty() {
            println!(
                "                 none live: parent PID {} has exited or was reused",
                root_proc.ppid
            );
        }
        for p in &chain {
            println!("               {}", describe(p));
        }
        if let Some(last) = chain.last()
            && last.ppid != 0
            && parent_of(processes, last).is_none()
        {
            println!(
                "                 (then parent PID {}, which has exited or was reused)",
                last.ppid
            );
        }

        let descendants: Vec<&Proc> = processes
            .iter()
            .filter(|p| p.pid != root.pid && is_in_tree(processes, root.pid, p))
            .collect();
        println!("descendants    (captured with the root in include mode)");
        if descendants.is_empty() {
            println!("                 none");
        }
        for p in &descendants {
            println!("               {}", describe(p));
        }

        println!("of interest    (relation to the root)");
        let interesting = |exe: &str| {
            let exe = exe.to_ascii_lowercase();
            exe.starts_with("discord")
                || exe.starts_with("leagueclient")
                || exe.starts_with("riotclient")
                || exe == GAME_PROCESS.to_ascii_lowercase()
                || exe == "vgc.exe"
                || exe == "vgtray.exe"
        };
        let mut any = false;
        let mut discord_in_tree = false;
        for p in processes.iter().filter(|p| interesting(&p.exe)) {
            any = true;
            let relation = if p.pid == root.pid {
                "the root"
            } else if is_in_tree(processes, root.pid, p) {
                if p.exe.to_ascii_lowercase().starts_with("discord") {
                    discord_in_tree = true;
                }
                "INSIDE the tree"
            } else if chain.iter().any(|a| a.pid == p.pid) {
                "ancestor, outside the tree"
            } else {
                "outside the tree"
            };
            println!("               {}  - {relation}", describe(p));
        }
        if !any {
            println!("                 none running");
        }
        if !processes
            .iter()
            .any(|p| p.exe.to_ascii_lowercase().starts_with("discord"))
        {
            println!(
                "               WARNING: Discord is not running. #7 needs it open and audible."
            );
        }
        if discord_in_tree {
            println!(
                "               WARNING: a Discord process is inside the root's tree. Include mode \
                 would capture it\n               \
                 by design, and the run could not show isolation. Check the root."
            );
        }
        let unverified = processes
            .iter()
            .filter(|p| p.created.is_none() && p.pid != 0)
            .count();
        if unverified > 0 {
            println!(
                "               ({unverified} process(es) could not be opened for a creation \
                 time; their parent links are\n               \
                 taken at face value.)"
            );
        }
        println!();
    }

    // --- Activation ------------------------------------------------------

    /// The completion handler `ActivateAudioInterfaceAsync` calls.
    ///
    /// The activation is asynchronous and there is no synchronous form, so the
    /// call below hands over this object and then blocks on an event it
    /// signals. The handler itself does nothing but signal: the result is read
    /// from the operation afterwards, on the calling thread, where the error
    /// can be reported in the one place that knows what was being attempted.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct ActivationHandler(HANDLE);

    impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
        fn ActivateCompleted(
            &self,
            _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
        ) -> WinResult<()> {
            // SAFETY: the event is owned by `activate`, which waits on it
            // before dropping it, and leaks it rather than close it if that
            // wait times out, so a late completion never signals a handle
            // value that has since been reused.
            unsafe { SetEvent(self.0) }
        }
    }

    /// Activates an `IAudioClient` bound to one process tree.
    fn activate(pid: u32, mode: Mode) -> Result<IAudioClient, String> {
        let activated = new_event()?;

        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    // **The tree, not the process.** A modern game is several
                    // processes and the one that plays audio is often not the
                    // one that owns the window. Targeting a single PID is how
                    // this spike would produce a false negative. There is no
                    // single-process mode to ask for anyway: these two are
                    // the only values the API has.
                    ProcessLoopbackMode: mode.api(),
                },
            },
        };

        // The activation parameters travel inside a PROPVARIANT as a raw blob,
        // built by hand because there is no constructor for VT_BLOB.
        //
        // **ManuallyDrop, because this PROPVARIANT does have a Drop.** windows-rs
        // adds one in `extensions/Win32/System/StructuredStorage.rs` that calls
        // `PropVariantClear`, which hands `pBlobData` to `CoTaskMemFree`. The
        // blob here points at `params` on the stack, so letting it drop freed a
        // stack address on the way out of `activate` and the process died with
        // 0xC0000374 (STATUS_HEAP_CORRUPTION) before capture began (#7).
        // Nothing here was allocated, so there is nothing to clear.
        let mut variant = ManuallyDrop::new(PROPVARIANT::default());
        // SAFETY: writing the active arm of a zeroed union, which is what the
        // callee reads given `vt = VT_BLOB`. `params` outlives the call.
        unsafe {
            let inner = &mut variant.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob = BLOB {
                cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: (&raw mut params).cast::<u8>(),
            };
        }

        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivationHandler(activated.0).into();

        // SAFETY: the device path is a static string, the IID is a static,
        // `variant` and the blob it points at live until after the wait
        // below, and the handler is a live COM object.
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&raw const *variant),
                &handler,
            )
        }
        .map_err(|e| format!("ActivateAudioInterfaceAsync failed: {e}"))?;

        // SAFETY: `activated` is a live event that only the handler signals.
        let wait = unsafe { WaitForSingleObject(activated.0, ACTIVATION_TIMEOUT_MS) };
        if wait != WAIT_OBJECT_0 {
            // The handler still holds this handle and may yet fire.
            std::mem::forget(activated);
            return Err(
                "the activation never completed. Nothing in this API reports why, so the \
                 next thing to check\nis whether the PID still exists."
                    .to_string(),
            );
        }

        let mut activate_result = windows::core::HRESULT(0);
        let mut interface: Option<windows::core::IUnknown> = None;
        // SAFETY: the operation has completed (the handler ran), and both
        // out-parameters are live.
        unsafe { operation.GetActivateResult(&mut activate_result, &mut interface) }
            .map_err(|e| format!("GetActivateResult failed: {e}"))?;
        activate_result.ok().map_err(|e| {
            format!(
                "activation was refused: {e}\n\
                 E_ACCESSDENIED here is the finding, not a bug: it means this Windows will not\n\
                 let this process capture that target. Record whether the run was elevated."
            )
        })?;

        interface
            .ok_or_else(|| "activation succeeded and returned no interface".to_string())?
            .cast::<IAudioClient>()
            .map_err(|e| format!("the activated object is not an IAudioClient: {e}"))
    }

    fn float_format() -> WAVEFORMATEX {
        let block_align = CHANNELS * BITS / 8;
        WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: CHANNELS,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * u32::from(block_align),
            nBlockAlign: block_align,
            wBitsPerSample: BITS,
            cbSize: 0,
        }
    }

    // --- Capture ---------------------------------------------------------

    fn capture(pid: u32, mode: Mode, seconds: u64, out: &Path) -> Result<(), String> {
        println!(
            "capture        mode {} on PID {pid}, {seconds}s -> {}",
            mode.name(),
            out.display()
        );
        let client = activate(pid, mode)?;

        let format = float_format();
        // Event-driven: the engine signals when a buffer is ready rather than
        // us polling a period. The real backend will be event-driven, and a
        // difference here would make the spike's behaviour say nothing about
        // the thing it is standing in for.
        //
        // `AUDCLNT_STREAMFLAGS_LOOPBACK` is required even though the
        // activation already said "process loopback": the flag is what makes
        // the client a capture client on a render stream.
        // SAFETY: `client` is live and `format` outlives the call.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                // Buffer duration and periodicity must both be 0 for a process
                // loopback client. Anything else fails with E_INVALIDARG, and
                // the error does not say which argument.
                0,
                0,
                &format,
                None,
            )
        }
        .map_err(|e| {
            format!(
                "IAudioClient::Initialize failed: {e}\n\
                 E_INVALIDARG here usually means the buffer duration or periodicity was not 0,\n\
                 or the format asked for is not {SAMPLE_RATE} Hz / {CHANNELS} ch / {BITS}-bit float."
            )
        })?;
        println!(
            "format         {SAMPLE_RATE} Hz, {CHANNELS} ch, {BITS}-bit float (asserted; the engine converts)"
        );

        let ready = new_event()?;
        // SAFETY: `ready` is a live event and outlives the client's use of it:
        // the client is stopped before `ready` drops.
        unsafe { client.SetEventHandle(ready.0) }
            .map_err(|e| format!("SetEventHandle failed: {e}"))?;

        // SAFETY: `client` is initialised, which GetService requires.
        let capture_client: IAudioCaptureClient = unsafe { client.GetService() }
            .map_err(|e| format!("could not get IAudioCaptureClient: {e}"))?;

        let mut wav = WavWriter::create(out, SAMPLE_RATE, CHANNELS, BITS)?;

        println!();
        println!("   sec  packets   frames  silent-pkts   peak dBFS    rms dBFS");
        // SAFETY: `client` is initialised with an event handle set.
        unsafe { client.Start() }.map_err(|e| format!("IAudioClient::Start failed: {e}"))?;
        let outcome = pump(&capture_client, &ready, &mut wav, seconds);
        // SAFETY: `client` is live; stopping a started client has no other
        // precondition, and a failure here changes nothing about the file.
        let _ = unsafe { client.Stop() };

        let stats = outcome?;
        let bytes = wav.finish()?;
        print_summary(pid, mode, seconds, out, bytes, &stats);
        Ok(())
    }

    /// One wall-clock second of capture.
    #[derive(Default, Clone)]
    struct Second {
        packets: u64,
        frames: u64,
        silent_packets: u64,
        peak: f32,
        sum_squares: f64,
        samples: u64,
    }

    impl Second {
        fn rms(&self) -> f64 {
            if self.samples == 0 {
                0.0
            } else {
                (self.sum_squares / self.samples as f64).sqrt()
            }
        }
        fn has_signal(&self) -> bool {
            dbfs(f64::from(self.peak)) > SIGNAL_FLOOR_DBFS
        }
    }

    struct Stats {
        seconds: Vec<Second>,
        discontinuities: u64,
    }

    fn dbfs(amplitude: f64) -> f64 {
        if amplitude <= 0.0 {
            f64::NEG_INFINITY
        } else {
            20.0 * amplitude.log10()
        }
    }

    fn fmt_db(value: f64) -> String {
        if value.is_finite() {
            format!("{value:>8.1}")
        } else {
            "    -inf".to_string()
        }
    }

    fn print_second(index: usize, s: &Second) {
        println!(
            "  {:>4}  {:>7}  {:>7}  {:>11}    {}    {}{}",
            index + 1,
            s.packets,
            s.frames,
            s.silent_packets,
            fmt_db(f64::from(s.peak)),
            fmt_db(s.rms()),
            if s.packets == 0 {
                "   (no packets)"
            } else {
                ""
            },
        );
    }

    fn pump(
        capture_client: &IAudioCaptureClient,
        ready: &OwnedHandle,
        wav: &mut WavWriter,
        seconds: u64,
    ) -> Result<Stats, String> {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(seconds);
        let mut stats = Stats {
            seconds: vec![Second::default(); seconds as usize],
            discontinuities: 0,
        };
        let mut printed = 0usize;

        loop {
            let now = Instant::now();
            // Print every second that has fully elapsed.
            let elapsed = (now.duration_since(start).as_secs() as usize).min(stats.seconds.len());
            while printed < elapsed {
                print_second(printed, &stats.seconds[printed]);
                printed += 1;
            }
            if now >= deadline {
                break;
            }

            // A timeout is a quiet stretch, not a failure: see WAIT_SLICE_MS.
            // SAFETY: `ready` is a live event.
            let wait = unsafe { WaitForSingleObject(ready.0, WAIT_SLICE_MS) };
            if wait != WAIT_OBJECT_0 {
                continue;
            }

            // One event can cover several packets. Draining is not an
            // optimisation: leaving a packet behind makes the next
            // `GetBuffer` return it late and the capture drift.
            loop {
                // SAFETY: the client is started and live.
                let available = unsafe { capture_client.GetNextPacketSize() }
                    .map_err(|e| format!("GetNextPacketSize failed: {e}"))?;
                if available == 0 {
                    break;
                }

                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                // SAFETY: all three out-parameters are live; the buffer is
                // released below before the next GetBuffer.
                unsafe { capture_client.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }
                    .map_err(|e| format!("GetBuffer failed: {e}"))?;

                let index = (Instant::now().duration_since(start).as_secs() as usize)
                    .min(stats.seconds.len() - 1);
                let second = &mut stats.seconds[index];
                second.packets += 1;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                if silent {
                    second.silent_packets += 1;
                }
                if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                    stats.discontinuities += 1;
                }

                if frames > 0 {
                    let samples = frames as usize * CHANNELS as usize;
                    second.frames += u64::from(frames);
                    second.samples += samples as u64;
                    if silent {
                        // A silent packet's buffer contents are undefined
                        // rather than zero. Writing it through would put noise
                        // in the file and the peak would stop meaning anything.
                        wav.write_silence(samples)?;
                    } else {
                        // SAFETY: the engine owns this buffer until
                        // ReleaseBuffer and promises `frames * channels`
                        // samples of the format Initialize accepted, which is
                        // 32-bit float, suitably aligned.
                        let pcm =
                            unsafe { std::slice::from_raw_parts(data as *const f32, samples) };
                        for &sample in pcm {
                            let magnitude = sample.abs();
                            if magnitude > second.peak {
                                second.peak = magnitude;
                            }
                            second.sum_squares += f64::from(sample) * f64::from(sample);
                        }
                        wav.write_samples(pcm)?;
                    }
                }

                // SAFETY: releases exactly the packet GetBuffer handed out.
                unsafe { capture_client.ReleaseBuffer(frames) }
                    .map_err(|e| format!("ReleaseBuffer failed: {e}"))?;
            }
        }

        Ok(stats)
    }

    fn print_summary(pid: u32, mode: Mode, seconds: u64, out: &Path, bytes: u32, stats: &Stats) {
        let frames: u64 = stats.seconds.iter().map(|s| s.frames).sum();
        let packets: u64 = stats.seconds.iter().map(|s| s.packets).sum();
        let silent: u64 = stats.seconds.iter().map(|s| s.silent_packets).sum();
        let peak = stats.seconds.iter().map(|s| s.peak).fold(0.0f32, f32::max);
        let (sum_squares, samples) = stats.seconds.iter().fold((0.0f64, 0u64), |(q, n), s| {
            (q + s.sum_squares, n + s.samples)
        });
        let rms = if samples == 0 {
            0.0
        } else {
            (sum_squares / samples as f64).sqrt()
        };
        let with_signal = stats.seconds.iter().filter(|s| s.has_signal()).count();
        let without_packets = stats.seconds.iter().filter(|s| s.packets == 0).count();
        let expected = u64::from(SAMPLE_RATE) * seconds;

        println!();
        println!("== result ==");
        println!("mode           {} (root PID {pid})", mode.name());
        println!("file           {} ({bytes} bytes)", out.display());
        println!(
            "frames         {frames} of {expected} for {seconds}s wall clock ({:.1}s of audio)",
            frames as f64 / f64::from(SAMPLE_RATE)
        );
        println!(
            "packets        {packets} ({silent} flagged silent, {} discontinuities)",
            stats.discontinuities
        );
        println!(
            "peak / rms     {} / {} dBFS",
            fmt_db(dbfs(f64::from(peak))).trim(),
            fmt_db(dbfs(rms)).trim()
        );
        println!(
            "seconds        {with_signal} of {seconds} above {SIGNAL_FLOOR_DBFS} dBFS, \
             {without_packets} with no packets at all"
        );
        println!();
        println!("{}", verdict(mode, frames, peak, with_signal));
    }

    /// What the numbers mean, said here rather than left to whoever reads them.
    ///
    /// A run that captures nothing and a run that captures silence are
    /// different findings with the same file size, and telling them apart
    /// later from a WAV is harder than saying so now.
    fn verdict(mode: Mode, frames: u64, peak: f32, with_signal: usize) -> String {
        let heard = match mode {
            Mode::Include => "the game and nothing else",
            Mode::Exclude => "Discord (and anything else playing) and NOT the game",
        };
        if frames == 0 {
            return "NOTHING CAPTURED. The activation succeeded and the engine produced no \
                    frames at all.\nThat is either a tree making no sound, or a tree whose \
                    audio does not route through\nthe loopback we attached to. Check the target \
                    was audible before reading this as a failure."
                .to_string();
        }
        if peak == 0.0 {
            return "SILENCE CAPTURED. Frames arrived and every sample was zero.\nThis is the \
                    interesting failure: the stream exists, so the activation and the format \
                    are right,\nand the target's audio is not reaching it. In include mode, \
                    check the root above first:\nthe design document's warning is that a wrong \
                    root produces exactly this."
                .to_string();
        }
        if with_signal == 0 {
            return format!(
                "ONLY NOISE CAPTURED. Nothing reached {SIGNAL_FLOOR_DBFS} dBFS in any second. \
                 Treat as silence\nunless the WAV says otherwise."
            );
        }
        format!(
            "AUDIO CAPTURED. Now play the file: it should hold {heard}.\nThe numbers above \
             cannot tell game from Discord; only listening can."
        )
    }

    // --- WAV -------------------------------------------------------------

    /// A RIFF writer, and nothing more.
    ///
    /// Float WAV rather than 16-bit PCM so that nothing in the spike can be
    /// blamed for what the file sounds like: there is no conversion between
    /// what the engine produced and what lands on disk.
    struct WavWriter {
        file: BufWriter<File>,
        data_bytes: u32,
    }

    impl WavWriter {
        fn create(path: &Path, rate: u32, channels: u16, bits: u16) -> Result<Self, String> {
            let file = File::create(path)
                .map_err(|e| format!("could not create {}: {e}", path.display()))?;
            let mut writer = Self {
                file: BufWriter::new(file),
                data_bytes: 0,
            };
            writer.write_header(rate, channels, bits)?;
            Ok(writer)
        }

        fn write_header(&mut self, rate: u32, channels: u16, bits: u16) -> Result<(), String> {
            let block_align = channels * bits / 8;
            let mut header = Vec::with_capacity(44);
            header.extend_from_slice(b"RIFF");
            header.extend_from_slice(&0u32.to_le_bytes()); // patched in finish()
            header.extend_from_slice(b"WAVEfmt ");
            header.extend_from_slice(&16u32.to_le_bytes());
            header.extend_from_slice(&WAVE_FORMAT_IEEE_FLOAT.to_le_bytes());
            header.extend_from_slice(&channels.to_le_bytes());
            header.extend_from_slice(&rate.to_le_bytes());
            header.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
            header.extend_from_slice(&block_align.to_le_bytes());
            header.extend_from_slice(&bits.to_le_bytes());
            header.extend_from_slice(b"data");
            header.extend_from_slice(&0u32.to_le_bytes()); // patched in finish()
            self.file
                .write_all(&header)
                .map_err(|e| format!("could not write the WAV header: {e}"))
        }

        fn write_samples(&mut self, samples: &[f32]) -> Result<(), String> {
            for &sample in samples {
                self.file
                    .write_all(&sample.to_le_bytes())
                    .map_err(|e| format!("could not write samples: {e}"))?;
            }
            self.data_bytes += (samples.len() * 4) as u32;
            Ok(())
        }

        fn write_silence(&mut self, samples: usize) -> Result<(), String> {
            let zeros = vec![0u8; samples * 4];
            self.file
                .write_all(&zeros)
                .map_err(|e| format!("could not write silence: {e}"))?;
            self.data_bytes += zeros.len() as u32;
            Ok(())
        }

        /// Patches the two sizes RIFF puts before the data it describes.
        fn finish(mut self) -> Result<u32, String> {
            self.file
                .flush()
                .map_err(|e| format!("could not flush the WAV: {e}"))?;
            let mut file = self
                .file
                .into_inner()
                .map_err(|e| format!("could not finish the WAV: {e}"))?;

            file.seek(SeekFrom::Start(4))
                .and_then(|_| file.write_all(&(36 + self.data_bytes).to_le_bytes()))
                .and_then(|()| file.seek(SeekFrom::Start(40)).map(|_| ()))
                .and_then(|()| file.write_all(&self.data_bytes.to_le_bytes()))
                .map_err(|e| format!("could not patch the WAV sizes: {e}"))?;
            Ok(self.data_bytes + 44)
        }
    }
}
