; Installer hooks, included by Tauri's generated installer.nsi through
; `bundle.windows.nsis.installerHooks` in tauri.windows.conf.json (#220).
;
; ## What they are for
;
; The libobs worker, `libobs\extprocess_recorder.exe`, is a second process with
; every DLL in `libobs\` loaded. Windows will not let an installer replace or
; delete a file a running process has loaded, so installing over a running
; worker fails on the first DLL it touches, or skips it and leaves an old DLL
; beside a new worker. Tauri's own "close the running app" check cannot see the
; worker: at the pinned CLI (2.11.4) `CheckIfAppIsRunning` finds and kills
; processes by the *name* `${MAINBINARYNAME}.exe` and nothing else.
;
; So both hooks stop the worker before the template touches a file, and the
; post-uninstall hook then removes whatever the template's deletes left in
; libobs\ (#308).
;
; The own backend's capture worker (#241) needs nothing here. It is the main
; executable run with `--capture-worker`, so its image name is
; `${MAINBINARYNAME}.exe` and the template's by-name kill ends it with the app
; and the daemon, before the executable they all run is replaced. It also lives
; in a kill-on-close job object the daemon owns, so the daemon dying ends it.
; The devtools build's worker is `ninja-recorder-dev.exe`, so the kill cannot
; reach across builds.
;
; ## Only this install's worker
;
; The release and devtools builds install side by side (DEVELOPMENT.md section 10),
; and both ship a worker called `extprocess_recorder.exe`. Killing by name
; would stop the other build's worker in the middle of its recording. So the
; worker is found by *path*, through the Restart Manager: registering
; `$INSTDIR\libobs\extprocess_recorder.exe` as a resource and asking
; `RmGetList` who is using it answers with exactly the processes running that
; file. They are then terminated by process id and waited for. No plugin is
; involved beyond NSIS's own System plug-in, which the template already uses.
;
; ## The app goes first
;
; The hook runs before the template's own check, and the app owns the worker.
; Stopping the worker under a live app would leave a recorder that looks up and
; cannot record, and if the person then pressed Cancel on the template's
; "close the app?" prompt that is exactly what they would be left with. So when
; the worker is running and the app is too, the hook asks the same question
; with the template's own strings, and a Cancel aborts before anything has been
; stopped. OK stops the app (also by path), then the worker. The template's
; check that follows then finds nothing and asks nothing.
;
; A silent (`/S`, which is how the in-app updater runs this) or passive (`/P`)
; install does not ask, the same as the template.
;
; ## Not verified
;
; No gate runs makensis on Linux: CI's Windows build job is the first thing
; that compiles this, and nothing in CI runs an installer.
; docs/windows-verification.md section 10 says how to check it on a box.

!define /ifndef NR_ERROR_MORE_DATA 234
; sizeof(RM_PROCESS_INFO): RM_UNIQUE_PROCESS (12) + WCHAR[256] (512)
; + WCHAR[64] (128) + four 32-bit fields (16). dwProcessId is at offset 0.
!define NR_RM_PROCESS_INFO_SIZE 668
; PROCESS_TERMINATE | SYNCHRONIZE
!define NR_TERMINATE_ACCESS 0x00100001
; How long to wait for each process to be gone once it has been told to go.
!define NR_EXIT_WAIT_MS 5000
!define NR_WORKER "libobs\extprocess_recorder.exe"

; Counts the processes running a file, and optionally terminates them.
;
;   Push "<full path>"
;   Push 0 | 1        ; 1 terminates them and waits for each to exit
;   Call NR_ProcessesUsing
;   Pop $R0           ; how many were running (0 if the query failed)
;
; Preserves $0-$9. A failure anywhere is a count of zero rather than an abort:
; this is a best effort in front of the template's own check, and it must never
; leave an install worse off than it was without it.
!macro NR_DEFINE_PROCESSES_USING un
Function ${un}NR_ProcessesUsing
  Exch $1 ; mode
  Exch
  Exch $0 ; path
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  Push $8
  Push $9

  StrCpy $8 0
  System::Call 'RSTRTMGR::RmStartSession(*i .r2, i 0, w .r3) i .r4'
  ; String comparison, not `=`: a call that could not be made leaves "error",
  ; which NSIS's integer comparison would read as 0, meaning success.
  ${If} $4 == 0
    System::Call 'RSTRTMGR::RmRegisterResources(i r2, i 1, *w r0, i 0, p 0, i 0, p 0) i .r4'
    ${If} $4 == 0
      ; Sized with no buffer first: ERROR_MORE_DATA and a count if anything
      ; is using the file, ERROR_SUCCESS and zero if nothing is.
      System::Call 'RSTRTMGR::RmGetList(i r2, *i .r5, *i 0 r6, p 0, *i .r7) i .r4'
      ${If} $4 = ${NR_ERROR_MORE_DATA}
      ${AndIf} $5 > 0
        StrCpy $8 $5
        ${If} $1 = 1
          ; Room for a few more, for a process that starts between the calls.
          IntOp $6 $5 + 4
          IntOp $9 $6 * ${NR_RM_PROCESS_INFO_SIZE}
          System::Alloc $9
          Pop $9
          ${If} $9 P<> 0
            System::Call 'RSTRTMGR::RmGetList(i r2, *i .r5, *i r6 r6, p r9, *i .r7) i .r4'
            ${If} $4 == 0
              StrCpy $8 $6
              StrCpy $3 0
              ${While} $3 < $6
                IntOp $4 $3 * ${NR_RM_PROCESS_INFO_SIZE}
                IntPtrOp $4 $9 + $4
                System::Call '*$4(i .r7)'
                System::Call 'kernel32::OpenProcess(i ${NR_TERMINATE_ACCESS}, i 0, i r7) p .r4'
                ${If} $4 P<> 0
                  System::Call 'kernel32::TerminateProcess(p r4, i 1) i .r5'
                  System::Call 'kernel32::WaitForSingleObject(p r4, i ${NR_EXIT_WAIT_MS}) i .r5'
                  System::Call 'kernel32::CloseHandle(p r4)'
                  DetailPrint "Stopped process $7 ($0)"
                ${Else}
                  DetailPrint "Could not stop process $7 ($0)"
                ${EndIf}
                IntOp $3 $3 + 1
              ${EndWhile}
            ${EndIf}
            System::Free $9
          ${EndIf}
        ${EndIf}
      ${EndIf}
    ${EndIf}
    System::Call 'RSTRTMGR::RmEndSession(i r2)'
  ${EndIf}

  ; The count goes back on the stack in place of the saved $0, and $0-$9
  ; come back as they were.
  StrCpy $0 $8
  Pop $9
  Pop $8
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Exch $0 ; $0 restored, count on top, saved $1 beneath it
  Exch
  Pop $1
FunctionEnd
!macroend

!insertmacro NR_DEFINE_PROCESSES_USING ""
!insertmacro NR_DEFINE_PROCESSES_USING "un."

; Stops this install's capture worker, and the app first if it is running.
;
; Expanded inside the template's Install and Uninstall sections, which is where
; $PassiveMode, $INSTDIR and the template's language strings all exist.
!macro NR_STOP_CAPTURE_WORKER un
  Push $R5
  Push $R6

  Push "$INSTDIR\${NR_WORKER}"
  Push 0
  Call ${un}NR_ProcessesUsing
  Pop $R5

  ${If} $R5 > 0
    Push "$INSTDIR\${MAINBINARYNAME}.exe"
    Push 0
    Call ${un}NR_ProcessesUsing
    Pop $R6

    ${If} $R6 > 0
      ${IfNot} ${Silent}
      ${AndIf} $PassiveMode <> 1
        nsis_tauri_utils::StrReplace "$(appRunningOkKill)" "{{product_name}}" "${PRODUCTNAME}"
        Pop $R6
        ${IfNot} ${Cmd} `MessageBox MB_OKCANCEL "$R6" IDOK`
          nsis_tauri_utils::StrReplace "$(appRunning)" "{{product_name}}" "${PRODUCTNAME}"
          Pop $R6
          Abort $R6
        ${EndIf}
      ${EndIf}

      Push "$INSTDIR\${MAINBINARYNAME}.exe"
      Push 1
      Call ${un}NR_ProcessesUsing
      Pop $R6
    ${EndIf}

    Push "$INSTDIR\${NR_WORKER}"
    Push 1
    Call ${un}NR_ProcessesUsing
    Pop $R6
  ${EndIf}

  Pop $R6
  Pop $R5
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro NR_STOP_CAPTURE_WORKER ""
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro NR_STOP_CAPTURE_WORKER "un."
!macroend

; How many times to try removing libobs\, and how long to wait between tries.
!define NR_RMDIR_TRIES 20
!define NR_RMDIR_WAIT_MS 500

; Removes $INSTDIR\libobs and everything in it (#308).
;
; The template's uninstall deletes resources one `Delete` per file, from the
; list of files it bundled, and ends with a non-recursive `RMDir` per
; directory. That leaves two kinds of file behind, and #308 saw both:
;
; - Anything the worker had loaded, if it was still running when the
;   `Delete`s ran. `Delete` on a mapped image fails and sets the error flag,
;   which the template never reads. What survived was exactly the worker's
;   module list: extprocess_recorder.exe and the 23 DLLs it loads, while
;   every file in the same folders it had not loaded (ffmpeg.exe,
;   obs-ffmpeg-mux.exe, libobs-opengl.dll, data\...) was deleted.
; - Anything written at run time, which is on no list. meta.json,
;   package.json and compatibility.json survived too, and no staged libobs
;   ships a .json file: those are the names of OBS win-capture's
;   compatibility-data download, so the worker most likely wrote them.
;
; The folder is ours alone, so the fix is to remove all of it once the worker
; is gone. This runs after the template's `Delete`s, so it only ever sees what
; they left. The worker is stopped by path again first, in case it outlived
; the pre-uninstall hook, and then the delete is retried for a few seconds: a
; worker whose app was killed under it exits by itself when its pipe closes,
; and until it does its DLLs cannot be deleted. The retry costs nothing when
; the first attempt succeeds, which is every uninstall with no worker running.
;
; Nothing here depends on $UpdateMode. The uninstaller only runs during an
; install when the reinstall page chooses "uninstall before installing", and
; the Install section that follows copies the whole libobs resource folder
; again. An in-app update (`/UPDATE`) never runs the uninstaller at all.
; App data lives under %APPDATA%\<identifier>, not $INSTDIR, and is untouched.
!macro NR_REMOVE_LIBOBS_DIR
  Push $R5
  Push $R6

  ${If} $INSTDIR != ""
  ${AndIf} ${FileExists} "$INSTDIR\libobs\*.*"
    Push "$INSTDIR\${NR_WORKER}"
    Push 1
    Call un.NR_ProcessesUsing
    Pop $R6

    StrCpy $R5 0
    ${Do}
      ClearErrors
      RMDir /r "$INSTDIR\libobs"
      ${IfNot} ${Errors}
      ${AndIfNot} ${FileExists} "$INSTDIR\libobs\*.*"
        ${Break}
      ${EndIf}
      IntOp $R5 $R5 + 1
      ${If} $R5 >= ${NR_RMDIR_TRIES}
        DetailPrint "Could not remove $INSTDIR\libobs; a file in it is still in use"
        ${Break}
      ${EndIf}
      Sleep ${NR_RMDIR_WAIT_MS}
    ${Loop}

    ; The template's own `RMDir "$INSTDIR"` ran while libobs\ was still
    ; there, so it failed. Non-recursive: this only removes an empty folder.
    RMDir "$INSTDIR"
  ${EndIf}

  Pop $R6
  Pop $R5
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro NR_REMOVE_LIBOBS_DIR
!macroend
