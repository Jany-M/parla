; Parla — NSIS hooks for the Tauri installer.
; Uninstall can optionally wipe EVERYTHING the app created: %LOCALAPPDATA%\Parla
; (models, runtime, meetings DB, settings, logs, webview profile) plus bundle-id
; folders Tauri/WebView2 may have used.
;
; Match Tauri's own rule: only delete app data when the user checks
; "Delete the application data" ($DeleteAppDataCheckboxState = 1) and this is
; not an update (/UPDATE → $UpdateMode = 1). Leaving the box unchecked (or
; silent uninstall without /DELETEAPPDATA) must keep Library transcripts.

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop a leftover language-model server — but only OURS (never another app's llama-server).
  nsExec::Exec `powershell -NoProfile -WindowStyle Hidden -Command "Get-Process llama-server -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$LOCALAPPDATA\Parla\*' } | Stop-Process -Force"`
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    ; give WebView2 a moment to release its profile files after the app was closed
    Sleep 1500
    RMDir /r "$LOCALAPPDATA\Parla"
    RMDir /r "$LOCALAPPDATA\${BUNDLEID}"
    RMDir /r "$APPDATA\${BUNDLEID}"
    ; leftover from older builds that used com.parla.app
    RMDir /r "$LOCALAPPDATA\com.parla.app"
    RMDir /r "$APPDATA\com.parla.app"
    ; anything still locked is removed at next reboot
    ${If} ${FileExists} "$LOCALAPPDATA\Parla"
      RMDir /r /REBOOTOK "$LOCALAPPDATA\Parla"
    ${EndIf}
  ${EndIf}
!macroend
