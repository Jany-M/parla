//! macOS capture permissions for mic + system/meeting audio.
//!
//! System audio uses Core Audio process taps (macOS 14.2+). Without **Screen Recording**
//! and **System Audio Recording** (TCC `AudioCapture`), the tap can start but deliver silence.
//! There is no public status API for system audio; we probe private TCC when available.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Denied / NotDetermined / Unknown are produced on macOS only.
pub enum PermState {
    Granted,
    Denied,
    NotDetermined,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapturePermissions {
    /// False on Windows/Linux: UI should hide the permission card.
    pub required: bool,
    pub microphone: PermState,
    pub screen_recording: PermState,
    pub system_audio: PermState,
}

#[cfg(not(target_os = "macos"))]
pub fn status() -> CapturePermissions {
    CapturePermissions {
        required: false,
        microphone: PermState::Granted,
        screen_recording: PermState::Granted,
        system_audio: PermState::Granted,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn request_missing() -> CapturePermissions {
    status()
}

#[cfg(not(target_os = "macos"))]
pub fn open_settings() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{CapturePermissions, PermState};
    use objc2::runtime::Bool;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    use std::ffi::c_void;
    use std::process::Command;
    use std::sync::OnceLock;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    type TccPreflight = unsafe extern "C" fn(*const c_void) -> i32;
    type TccRequest = unsafe extern "C" fn(*const c_void, Option<extern "C" fn(Bool)>);

    struct Tcc {
        preflight: Option<TccPreflight>,
        request: Option<TccRequest>,
        /// Retained NSString / CFString for the AudioCapture TCC service.
        audio_service: *const c_void,
    }

    // SAFETY: TCC symbols are process-global; only called from the Parla UI thread / commands.
    unsafe impl Send for Tcc {}
    unsafe impl Sync for Tcc {}

    fn tcc() -> &'static Tcc {
        static TCC: OnceLock<Tcc> = OnceLock::new();
        TCC.get_or_init(|| {
            let mut preflight = None;
            let mut request = None;
            let mut audio_service = std::ptr::null();
            for path in [
                "/System/Library/PrivateFrameworks/TCC.framework/TCC",
                "/System/Library/Frameworks/TCC.framework/TCC",
            ] {
                let Ok(lib) = libloading::Library::new(path) else { continue };
                let lib = Box::leak(Box::new(lib));
                unsafe {
                    preflight = lib.get::<TccPreflight>(b"TCCAccessPreflight\0").ok().map(|s| *s);
                    request = lib.get::<TccRequest>(b"TCCAccessRequest\0").ok().map(|s| *s);
                }
                if preflight.is_some() {
                    break;
                }
            }
            if preflight.is_some() {
                unsafe {
                    let name = NSString::from_str("AudioCapture");
                    let retained: *const NSString = msg_send![&*name, retain];
                    audio_service = retained as *const c_void;
                }
            }
            Tcc { preflight, request, audio_service }
        })
    }

    fn mic_status() -> PermState {
        unsafe {
            let media = NSString::from_str("soun");
            let status: i32 = msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*media];
            match status {
                3 => PermState::Granted,
                2 | 1 => PermState::Denied,
                0 => PermState::NotDetermined,
                _ => PermState::Unknown,
            }
        }
    }

    fn request_mic() {
        unsafe {
            let media = NSString::from_str("soun");
            type Completion = Option<extern "C" fn(Bool)>;
            let completion: Completion = None;
            let _: () = msg_send![
                class!(AVCaptureDevice),
                requestAccessForMediaType: &*media,
                completionHandler: completion
            ];
        }
    }

    fn screen_status() -> PermState {
        if unsafe { CGPreflightScreenCaptureAccess() } {
            PermState::Granted
        } else {
            PermState::Denied
        }
    }

    fn request_screen() {
        let _ = unsafe { CGRequestScreenCaptureAccess() };
    }

    fn system_audio_status() -> PermState {
        let t = tcc();
        let Some(preflight) = t.preflight else {
            return PermState::Unknown;
        };
        if t.audio_service.is_null() {
            return PermState::Unknown;
        }
        match unsafe { preflight(t.audio_service) } {
            0 => PermState::Granted,
            1 => PermState::Denied,
            2 => PermState::NotDetermined,
            _ => PermState::Unknown,
        }
    }

    fn request_system_audio() {
        let t = tcc();
        if let (Some(request), svc) = (t.request, t.audio_service) {
            if !svc.is_null() {
                unsafe { request(svc, None) };
            }
        }
    }

    pub fn status() -> CapturePermissions {
        CapturePermissions {
            required: true,
            microphone: mic_status(),
            screen_recording: screen_status(),
            system_audio: system_audio_status(),
        }
    }

    pub fn request_missing() -> CapturePermissions {
        let s = status();
        if s.microphone != PermState::Granted {
            request_mic();
        }
        if s.screen_recording != PermState::Granted {
            request_screen();
        }
        if !matches!(s.system_audio, PermState::Granted) {
            request_system_audio();
        }
        status()
    }

    pub fn open_settings() -> Result<(), String> {
        for pane in [
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        ] {
            Command::new("open").arg(pane).spawn().map_err(|e| e.to_string())?;
        }
        let _ = Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture")
            .spawn();
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use macos::{open_settings, request_missing, status};
