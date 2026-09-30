//! Native capture backends. Environment detection is only a hint, not a capability probe.
use std::{fmt, str::FromStr};

use lisnap_core::{BackendDescriptor, BackendStatus, CaptureBackend, CaptureError};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod stub;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Wayland,
    X11,
    Windows,
    MacOS,
}

impl BackendKind {
    pub const ALL: [Self; 4] = [Self::Wayland, Self::X11, Self::Windows, Self::MacOS];

    pub fn descriptor(self) -> BackendDescriptor {
        let (id, name, technology, supported) = match self {
            Self::Wayland => (
                "wayland",
                "Wayland native",
                "ext-image-copy-capture / wlr-screencopy (planned)",
                cfg!(target_os = "linux"),
            ),
            Self::X11 => ("x11", "X11 native", "X11 SHM / GetImage (planned)", cfg!(target_os = "linux")),
            Self::Windows => {
                ("windows", "Windows native", "Windows Graphics Capture (planned)", cfg!(target_os = "windows"))
            }
            Self::MacOS => ("macos", "macOS native", "ScreenCaptureKit (planned)", cfg!(target_os = "macos")),
        };
        BackendDescriptor {
            id,
            name,
            technology,
            status: if supported { BackendStatus::NotImplemented } else { BackendStatus::Unsupported },
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.descriptor().id)
    }
}

impl FromStr for BackendKind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.descriptor().id == value)
            .ok_or_else(|| format!("unknown backend '{value}'; use auto, wayland, x11, windows, or macos"))
    }
}

pub fn create_backend(kind: BackendKind) -> Result<Box<dyn CaptureBackend>, CaptureError> {
    match kind {
        #[cfg(target_os = "linux")]
        BackendKind::Wayland => Ok(linux::wayland::create()),
        #[cfg(target_os = "linux")]
        BackendKind::X11 => Ok(linux::x11::create()),
        #[cfg(target_os = "windows")]
        BackendKind::Windows => Ok(windows::create()),
        #[cfg(target_os = "macos")]
        BackendKind::MacOS => Ok(macos::create()),
        _ => Err(CaptureError::Unsupported(format!("{kind} cannot run on {}", std::env::consts::OS))),
    }
}

/// Select a candidate; actual protocol support and permissions must be negotiated later.
pub fn default_backend() -> Option<BackendKind> {
    #[cfg(target_os = "linux")]
    {
        let session = std::env::var("XDG_SESSION_TYPE").ok();
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
        let x11 = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
        linux_backend_hint(session.as_deref(), wayland, x11)
    }
    #[cfg(target_os = "windows")]
    {
        Some(BackendKind::Windows)
    }
    #[cfg(target_os = "macos")]
    {
        Some(BackendKind::MacOS)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_backend_hint(session: Option<&str>, wayland: bool, x11: bool) -> Option<BackendKind> {
    match session {
        Some("wayland") => Some(BackendKind::Wayland),
        Some("x11") => Some(BackendKind::X11),
        _ if wayland => Some(BackendKind::Wayland),
        _ if x11 => Some(BackendKind::X11),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lisnap_core::CaptureRequest;

    #[test]
    fn stub_backends_never_report_capture_success() {
        for kind in BackendKind::ALL {
            if kind.descriptor().status == BackendStatus::NotImplemented {
                let mut backend = create_backend(kind).unwrap();
                assert_eq!(backend.descriptor().id, kind.descriptor().id);
                assert!(matches!(backend.screens(), Err(CaptureError::NotImplemented(_))));
                assert!(matches!(backend.capture(&CaptureRequest::default()), Err(CaptureError::NotImplemented(_))));
            }
        }
    }

    #[test]
    fn foreign_platforms_are_rejected() {
        for kind in BackendKind::ALL {
            if kind.descriptor().status == BackendStatus::Unsupported {
                assert!(matches!(create_backend(kind), Err(CaptureError::Unsupported(_))));
            }
        }
    }

    #[test]
    fn parses_backend_names() {
        for kind in BackendKind::ALL {
            assert_eq!(kind.to_string().parse(), Ok(kind));
        }
        assert!("invalid".parse::<BackendKind>().is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wayland_does_not_fall_back_to_xwayland_capture() {
        assert_eq!(linux_backend_hint(Some("wayland"), true, true), Some(BackendKind::Wayland));
        assert_eq!(linux_backend_hint(Some("wayland"), false, true), Some(BackendKind::Wayland));
        assert_eq!(linux_backend_hint(Some("x11"), true, true), Some(BackendKind::X11));
        assert_eq!(linux_backend_hint(None, true, true), Some(BackendKind::Wayland));
        assert_eq!(linux_backend_hint(None, false, true), Some(BackendKind::X11));
        assert_eq!(linux_backend_hint(None, false, false), None);
    }
}
