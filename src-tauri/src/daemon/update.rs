//! Checking for updates, from the process that can refuse one. WS3 task 3.6.
//!
//! `tauri-plugin-updater` did this in the UI, and it needs an `AppHandle`, so
//! the daemon cannot use it. It also should not: the question "may this install
//! run now" is answered by whether a game is being recorded, and the daemon is
//! the process that knows.
//!
//! ## The check half only
//!
//! This fetches the manifest, decides what it means, records it and publishes
//! it. It does not download or install anything. The install half is the rest
//! of WS3.6: `reqwest` to fetch the installer, `minisign-verify` against the
//! baked public key, then run it with `/S` once nothing is recording.
//!
//! Until that lands, `install_update` refuses. It refused before this too, and
//! for a worse reason: since WS3.4 every command runs in the daemon, whose
//! update seam nothing had ever filled in, so an install attempt reached a
//! `None` and said "not available in this build" on a build where it was.
//!
//! ## What the frontend sees
//!
//! Both the stored result and an event. `get_update_status` answers from the
//! `Ctx` cell, which is what the About block reads on demand; `Event::UpdateStatus`
//! is pushed so a window open at the time does not have to poll. They carry the
//! same value by construction, because this is the only thing that sets it.

use std::sync::Arc;
use std::time::Duration;

use crate::contract::events::Event;
use crate::core::Ctx;
use crate::daemon::snapshot::Stream;
use crate::update::{self, CheckResult};
use crate::{info, warn};

/// How long after startup the first check runs.
///
/// Late enough that it is never competing with the recorder backend coming up,
/// the database opening or the pipe being bound, none of which should wait on a
/// network round trip to GitHub.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);

/// And how often after that.
///
/// Deliberately slack: CI publishes a release for every commit that lands on
/// `main`, so "something newer exists" is true most days, and a tighter loop
/// would only rediscover the same answer.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// How long to wait on the endpoint before giving up.
///
/// A check that hangs is worse than one that fails: the status stays `Checking`
/// forever and the About block shows a spinner with nothing behind it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Starts the periodic check, for the life of the daemon.
///
/// Returns immediately. Every outcome is recorded rather than returned: a
/// failed check is a *state* the About block renders, not an error to
/// propagate. Someone's network being down is not a bug.
pub fn spawn_checks(ctx: Arc<Ctx>, events: Stream) {
    if !crate::updates_enabled() {
        // A devtools build that updated itself would replace itself with the
        // production app (DEVELOPMENT.md §14). Said once, and recorded as a
        // state rather than left looking like a check that never answered.
        info!("update", "devtools build: updates are off");
        publish(&ctx, &events, CheckResult::Unsupported);
        return;
    }

    tokio::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let found = check(&ctx).await;
            publish(&ctx, &events, found);
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// One check: fetch the channel's manifest and decide what it means.
pub async fn check(ctx: &Arc<Ctx>) -> CheckResult {
    check_endpoint(&endpoint_for_channel(ctx)).await
}

/// One check against a named endpoint, which is the half a test can drive.
async fn check_endpoint(endpoint: &str) -> CheckResult {

    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(e) => return CheckResult::Failed(format!("Could not check for updates: {e}")),
    };

    let response = match client.get(endpoint).send().await {
        Ok(response) => response,
        Err(e) => return CheckResult::Failed(format!("Could not check for updates: {e}")),
    };
    if !response.status().is_success() {
        // A 404 on the alpha endpoint is the ordinary state of a repository
        // that has not published one yet, and it is not worth alarming anybody
        // about. Reported as a failure rather than as "nothing newer" all the
        // same: the two are different, and a silently missing endpoint is how
        // an app stops updating without anyone noticing.
        return CheckResult::Failed(format!(
            "Could not check for updates: the update server answered {}",
            response.status()
        ));
    }

    match response.text().await {
        Ok(body) => update::evaluate(&body, update::current_version()),
        Err(e) => CheckResult::Failed(format!("Could not check for updates: {e}")),
    }
}

/// Which manifest to read, from the user's channel preference.
///
/// A preference read that fails falls back to stable rather than propagating:
/// the conservative channel is the right answer to "we could not tell", and the
/// alternative is an install that stops checking because its settings table
/// hiccuped.
fn endpoint_for_channel(ctx: &Arc<Ctx>) -> String {
    let stored = ctx
        .db
        .get_ui_prefs()
        .ok()
        .and_then(|prefs| prefs.get(update::CHANNEL_PREF_KEY).cloned());

    match update::Channel::from_pref(stored.as_deref()) {
        update::Channel::Alpha => update::ALPHA_ENDPOINT.to_string(),
        update::Channel::Stable => update::STABLE_ENDPOINT.to_string(),
    }
}

/// Records what a check found, and tells anyone listening.
///
/// The two go together every time: a stored result nothing is told about is a
/// status the About block shows six hours late.
fn publish(ctx: &Arc<Ctx>, events: &Stream, found: CheckResult) {
    match &found {
        CheckResult::Found(offer) => info!("update", "update available: {}", offer.version),
        CheckResult::Failed(why) => warn!("update", "{why}"),
        _ => {}
    }
    ctx.set_update_check_result(found);

    // Rendered through the same function the command uses, so the event and a
    // later `get_update_status` cannot disagree about what is installable: that
    // answer depends on whether a game is running, which changes without
    // anything here being told.
    match crate::core::get_update_status(ctx) {
        Ok(status) => events.publish(Event::UpdateStatus { status }),
        Err(e) => warn!("update", "could not render the update status: {e}"),
    }
}

// --- Installing one --------------------------------------------------------
//
// The half that replaces the binary, and the reason the whole updater belongs
// in this process: "may this run now" is answered by whether a game is being
// recorded, and nothing else knows.

/// The seam `core::install_update` and `core::check_for_update` pull.
///
/// Whatever is here **must return immediately**. `core::install_update` calls
/// it and answers the frontend, which has already moved its row to
/// "Downloading" and has no other way to learn that this did not happen. So
/// each arm spawns and returns.
///
/// Takes a `Weak` because `Ctx` owns this closure and the closure needs the
/// `Ctx`: an owning handle would be a cycle neither end ever drops. The caller
/// builds the `Arc` with `Arc::new_cyclic`, which is what makes a `Weak`
/// available before the value it points at exists.
pub fn requester(
    ctx: std::sync::Weak<Ctx>,
    events: Stream,
) -> Box<dyn Fn(update::UpdateRequest) + Send + Sync> {
    Box::new(move |request| {
        let Some(ctx) = ctx.upgrade() else { return };
        let events = events.clone();
        match request {
            update::UpdateRequest::Check => {
                tokio::spawn(async move {
                    let found = check(&ctx).await;
                    publish(&ctx, &events, found);
                });
            }
            update::UpdateRequest::Install => {
                tokio::spawn(async move { install(&ctx, &events).await });
            }
        }
    })
}

/// Fetches, verifies and runs the installer. Does not return on success.
///
/// Every path that is not a successful launch records a status and publishes
/// it, because the frontend moved its own row to "Downloading" the moment the
/// button was pressed and this is the only thing that can move it back.
pub async fn install(ctx: &Arc<Ctx>, events: &Stream) {
    // Re-checked rather than taken from the last poll. The manifest owns the
    // download URL and its signature, and holding one for up to six hours
    // across a release means installing something the endpoint has moved on
    // from.
    let endpoint = endpoint_for_channel(ctx);
    let platform = match fetch_platform(&endpoint).await {
        Ok(platform) => platform,
        Err(why) => return fail(ctx, events, why),
    };

    info!("update", "downloading {}", platform.url);
    let bytes = match download(&platform.url).await {
        Ok(bytes) => bytes,
        Err(why) => return fail(ctx, events, why),
    };

    // Before anything is written where it could be run. A download that does
    // not verify is not an update, it is whatever happened to be served.
    if let Err(why) = update::verify(&bytes, &platform.signature) {
        return fail(ctx, events, why);
    }
    info!("update", "signature verified, {} bytes", bytes.len());

    let installer = match write_installer(&platform.url, &bytes) {
        Ok(path) => path,
        Err(why) => return fail(ctx, events, why),
    };

    // Said before the process goes away, so a connected UI shows "an update is
    // being installed" rather than a dead pipe. The grace in `daemon::finish`
    // is not available here: the installer replaces this binary, so there is no
    // orderly shutdown to run afterwards.
    events.publish(Event::DaemonShuttingDown {
        reason: crate::contract::events::ShutdownReason::Update,
    });
    tokio::time::sleep(Duration::from_millis(250)).await;

    // The recording, if there is one, gets finalized first. The gate in
    // `core::install_update` refuses while one is in flight, so this is the
    // narrow case where a game started between the click and here.
    let supervisor = Arc::clone(&ctx.supervisor);
    let _ = tokio::task::spawn_blocking(move || supervisor.finalize_for_shutdown()).await;

    info!("update", "handing over to {}", installer.display());
    match launch_installer(&installer) {
        // The installer replaces this binary and restarts the app, so there is
        // nothing left for this process to do and nothing that should keep it
        // alive while the file it is running from is replaced.
        Ok(()) => std::process::exit(0),
        Err(why) => fail(ctx, events, why),
    }
}

/// The current manifest's entry for this platform.
async fn fetch_platform(endpoint: &str) -> Result<update::ManifestPlatform, String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Could not reach the update server: {e}"))?;
    let body = client
        .get(endpoint)
        .send()
        .await
        .map_err(|e| format!("Could not reach the update server: {e}"))?
        .text()
        .await
        .map_err(|e| format!("Could not read the update manifest: {e}"))?;

    let manifest: update::Manifest = serde_json::from_str(&body)
        .map_err(|e| format!("Could not read the update manifest: {e}"))?;
    manifest
        .platforms
        .get(update::PLATFORM)
        .cloned()
        .ok_or_else(|| format!("This release has nothing for {}", update::PLATFORM))
}

/// The artifact, in memory.
///
/// Held rather than streamed to disk because it has to be verified as a whole
/// before any of it is written somewhere it could be executed, and an NSIS
/// bundle is tens of megabytes rather than hundreds.
async fn download(url: &str) -> Result<Vec<u8>, String> {
    // No timeout on this one, unlike the check: a slow connection downloading
    // 60 MB is not a failure, and cutting it off at twenty seconds would make
    // updates impossible on exactly the connections that most need them to be
    // resumable rather than abandoned.
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Could not download the update: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Could not download the update: the server answered {}", response.status()));
    }
    response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("Could not download the update: {e}"))
}

/// Writes the verified download somewhere it can be run from.
///
/// **It is the installer, not an archive containing one.** This unzipped until
/// it was run for the first time, on the strength of a comment saying Tauri's
/// NSIS updater artifact is a zip with the setup executable inside it. That was
/// Tauri v1's shape. Since v2 the updater artifact *is* the installer:
/// `createUpdaterArtifacts` emits `<app>_<version>_x64-setup.exe` beside a
/// `.sig` that signs those exact bytes, and the manifest's `url` points
/// straight at the `.exe`. So the download was a PE, `ZipArchive::new` went
/// looking for an end-of-central-directory record that a PE does not have, and
/// every install ended at "The update is not a readable archive: invalid Zip
/// archive: Could not find EOCD".
///
/// The signature is what makes writing these bytes safe, and it has already
/// been checked by the time this runs. Dropping the unzip also drops a class of
/// problem with it: an archive's entry names are remote input and had to be
/// defended against naming a path outside the directory, whereas a file name
/// derived from our own manifest's URL and reduced to its final component has
/// nowhere else to go.
fn write_installer(url: &str, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    // A directory of our own, named for the process, so two daemons cannot
    // write the same file and an installer left behind by a failed attempt is
    // findable rather than anonymous.
    let dir = std::env::temp_dir().join(format!("ninja-recorder-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not prepare {}: {e}", dir.display()))?;

    // The URL is ours, from our own manifest, but only its final component is
    // used and only if it looks like an executable. Windows runs a file by its
    // extension, so a name that arrived without one would produce a file
    // nothing could launch and an error pointing at the wrong thing.
    let name = url
        .rsplit('/')
        .next()
        .filter(|name| name.to_ascii_lowercase().ends_with(".exe"))
        .and_then(|name| std::path::Path::new(name).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("ninja-recorder-update.exe");
    let path = dir.join(name);

    std::fs::write(&path, bytes).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// Runs the installer and returns, leaving it to replace this binary.
///
/// `/S` is NSIS's silent mode, `/UPDATE` is what the bundle's own script reads
/// to know it is replacing an install rather than making one, and `/R` is what
/// starts the app again afterwards. Detached, for the reason `daemon::spawn`
/// detaches: the process it belongs to is about to stop existing.
///
/// ## `/R` was missing, and the update looked like it had killed the app
///
/// Without it the install completed and nothing came back: no daemon, no
/// window, and no recorder until someone started it by hand (#145). The three
/// flags are parsed by the generated `installer.nsi`, so this list is a
/// contract with the bundle rather than with NSIS in general.
///
/// **`/R` with no `/ARGS`, deliberately.** It relaunches the main binary with
/// no arguments, which is `Launch::Ui`: a window. The daemon is what was
/// updating and the daemon is what must exist afterwards, so `/ARGS --daemon`
/// looks like the more correct answer and is the worse one. The person
/// pressed Install in a window and watched it vanish; bringing back only an
/// invisible background process reads as a failed update. A window comes back,
/// `connect_or_start` starts a daemon because none is listening, and both
/// exist a second later.
///
/// **`/S` rather than `/P`, also deliberately.** The bundle's script parses
/// `/P` for passive mode, which would show a progress bar instead of nothing,
/// and `tauri.conf.json` still carries an `installMode: "passive"` that has
/// been dead since the daemon took the install over. Passive is arguably the
/// nicer experience. It is not worth trading for it here: silent is what has
/// now been observed installing correctly on real hardware, and the way to
/// change it is to verify the change, not to assume it.
fn launch_installer(path: &std::path::Path) -> Result<(), String> {
    let mut command = std::process::Command::new(path);
    command.args(["/S", "/UPDATE", "/R"]);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(DETACHED_PROCESS);
    }

    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start the installer: {e}"))
}

/// Records a failure and tells the frontend, which is waiting on one answer or
/// the other.
fn fail(ctx: &Arc<Ctx>, events: &Stream, why: String) {
    warn!("update", "{why}");
    publish(ctx, events, CheckResult::Failed(why));
}

#[cfg(test)]
mod tests {
    //! Against a local HTTP server rather than GitHub.
    //!
    //! What is worth testing here is the *decisions* either side of the
    //! network: which endpoint gets asked, what a manifest means, and whether a
    //! download that does not verify is refused. A test that reached the real
    //! endpoint would be testing GitHub's uptime and would go red the day a
    //! release was cut.

    use super::*;
    use crate::db::Db;
    use crate::recorder::Recorder;
    use crate::recorder::stub::StubRecorder;
    use crate::state_machine;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn ctx() -> Arc<Ctx> {
        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(StubRecorder::new())));
        let db = Arc::new(Db::open_temporary().unwrap());
        let dir = std::env::temp_dir().join(format!("nr-update-test-{}", std::process::id()));
        let supervisor =
            state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));
        Arc::new(Ctx::new(recorder, supervisor, db, dir.clone(), dir.join("ddragon"), None))
    }

    /// Serves one response and returns the URL it is at.
    ///
    /// Hand-rolled rather than a test-server crate: this is four lines of HTTP
    /// and the alternative is a dependency in the shipped tree for the benefit
    /// of one test module.
    async fn serve_once(status: &'static str, body: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut scratch = [0u8; 1024];
            let _ = socket.read(&mut scratch).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{addr}/manifest.json")
    }

    #[tokio::test]
    async fn a_manifest_offering_something_newer_is_an_offer() {
        let body = format!(
            r#"{{"version":"999.0.0","platforms":{{"{}":{{"signature":"s","url":"u"}}}}}}"#,
            update::PLATFORM
        );
        let url = serve_once("200 OK", body).await;

        let platform = fetch_platform(&url).await.expect("the manifest names this platform");
        assert_eq!(platform.url, "u");
    }

    /// A release with no Windows bundle is a real state, and the message has to
    /// name the platform rather than say "not found".
    #[tokio::test]
    async fn a_manifest_without_this_platform_says_which_one_is_missing() {
        let body =
            r#"{"version":"999.0.0","platforms":{"linux-x86_64":{"signature":"s","url":"u"}}}"#;
        let url = serve_once("200 OK", body.to_string()).await;

        let error = fetch_platform(&url).await.expect_err("nothing for us here");
        assert!(error.contains(update::PLATFORM), "got: {error}");
    }

    /// The endpoint being gone is the failure that would otherwise be silent:
    /// an app that stops updating and never says so.
    #[tokio::test]
    async fn a_404_is_reported_rather_than_read_as_nothing_newer() {
        let url = serve_once("404 Not Found", "no".to_string()).await;
        let found = check_endpoint(&url).await;
        assert!(
            matches!(&found, CheckResult::Failed(why) if why.contains("404")),
            "got: {found:?}"
        );
    }

    /// The channel preference decides which manifest is read, and a missing or
    /// unreadable preference has to mean stable rather than nothing.
    #[test]
    fn the_channel_preference_chooses_the_endpoint() {
        let ctx = ctx();
        assert_eq!(endpoint_for_channel(&ctx), update::STABLE_ENDPOINT, "default is stable");

        ctx.db.set_ui_pref(update::CHANNEL_PREF_KEY, "alpha").unwrap();
        assert_eq!(endpoint_for_channel(&ctx), update::ALPHA_ENDPOINT);

        ctx.db.set_ui_pref(update::CHANNEL_PREF_KEY, "nonsense").unwrap();
        assert_eq!(
            endpoint_for_channel(&ctx),
            update::STABLE_ENDPOINT,
            "an unreadable preference must fall back to the conservative channel"
        );
    }

    /// The name comes from our own manifest, and only its last component is
    /// used. Nothing here is a defence against a hostile URL, because a hostile
    /// manifest would have had to be signed; it is a defence against a URL that
    /// is merely *odd*, and against writing a file Windows would refuse to run.
    #[test]
    fn the_installer_lands_under_temp_with_a_runnable_name() {
        let path = write_installer(
            "https://example.invalid/releases/download/v9/../../ninja_9_x64-setup.exe",
            b"not really an installer",
        )
        .expect("the bytes are written");

        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("ninja_9_x64-setup.exe"),
            "only the final component of the URL is used"
        );
        assert!(
            path.starts_with(std::env::temp_dir()),
            "it must land under the temp directory: {}",
            path.display()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"not really an installer");
        let _ = std::fs::remove_file(path);
    }

    /// A URL that does not end in `.exe` still has to produce something
    /// runnable, because Windows decides that by extension and the failure
    /// otherwise arrives as "the installer would not start", nowhere near here.
    #[test]
    fn a_url_without_an_exe_name_still_writes_something_runnable() {
        let path = write_installer("https://example.invalid/download?id=7", b"bytes")
            .expect("the bytes are written");

        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some("exe"),
            "a file Windows will not launch is not an installer"
        );
        let _ = std::fs::remove_file(path);
    }
}
