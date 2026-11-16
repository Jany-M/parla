; Parla — full-removal hooks for the Tauri NSIS installer.
; Uninstall removes EVERYTHING the app created: the app folder (done by the template), plus
; %LOCALAPPDATA%\Parla (downloaded models, runtime, meetings database, settings, logs, webview
; profile) and the bundle-id folders Tauri/WebView2 may have used. Updates (/UPDATE) keep data.

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
  ${If} $UpdateMode <> 1
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
