#[cfg(target_os = "linux")]
mod linux {
    use std::{error::Error, path::PathBuf};

    use lisnap_core::CaptureRequest;
    use lisnap_platform::{
        WaylandOverlayOutcome, capture_wayland, capture_wayland_socket, run_wayland_frozen_overlay,
        run_wayland_frozen_overlay_socket,
    };

    #[derive(Debug, Default, PartialEq, Eq)]
    struct Options {
        socket: Option<PathBuf>,
        screen_id: Option<String>,
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        let options = parse(std::env::args().skip(1))?;
        let request =
            CaptureRequest { screen_id: options.screen_id.clone(), include_cursor: false, cancellation: None };
        let frames = match options.socket.as_deref() {
            Some(path) => capture_wayland_socket(path, &request)?,
            None => capture_wayland(&request)?,
        };
        let frame = frames.into_iter().next().ok_or("Wayland capture returned no frame for overlay validation")?;
        let size = frame.image.size();
        eprintln!("LiSnap overlay validation: captured {} ({}x{})", frame.screen.id, size.width, size.height);

        let outcome = match options.socket.as_deref() {
            Some(path) => run_wayland_frozen_overlay_socket(path, &frame)?,
            None => run_wayland_frozen_overlay(&frame)?,
        };
        match outcome {
            WaylandOverlayOutcome::Selected(rect) => {
                let crop = frame.image.crop(rect)?;
                let crop_size = crop.size();
                println!(
                    "selected {}x{} at {},{} | {} RGBA bytes | fnv64 {:016x}",
                    rect.width,
                    rect.height,
                    rect.x,
                    rect.y,
                    crop.pixels().len(),
                    fnv1a64(crop.pixels())
                );
                if crop_size.width != rect.width || crop_size.height != rect.height {
                    return Err("cropped image dimensions do not match overlay selection".into());
                }
                Ok(())
            }
            WaylandOverlayOutcome::Cancelled => {
                println!("selection cancelled");
                Ok(())
            }
            WaylandOverlayOutcome::Closed => Err("compositor closed the selection overlay".into()),
        }
    }

    fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut options = Options::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--socket" => {
                    if options.socket.is_some() {
                        return Err("--socket may only be specified once".into());
                    }
                    options.socket = Some(PathBuf::from(args.next().ok_or("--socket requires a path")?));
                }
                "--screen" => {
                    if options.screen_id.is_some() {
                        return Err("--screen may only be specified once".into());
                    }
                    options.screen_id = Some(args.next().ok_or("--screen requires an output ID")?);
                }
                "-h" | "--help" => {
                    return Err("usage: wayland-overlay [--socket PATH] [--screen OUTPUT]".into());
                }
                _ => return Err(format!("unknown argument '{arg}'")),
            }
        }
        Ok(options)
    }

    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_socket_and_screen() {
            assert_eq!(
                parse(["--socket".into(), "/run/user/1000/wayland-1".into(), "--screen".into(), "DP-1".into(),])
                    .unwrap(),
                Options { socket: Some(PathBuf::from("/run/user/1000/wayland-1")), screen_id: Some("DP-1".into()) }
            );
        }

        #[test]
        fn rejects_duplicate_and_unknown_arguments() {
            assert!(parse(["--screen".into(), "DP-1".into(), "--screen".into(), "DP-2".into()]).is_err());
            assert!(parse(["--unknown".into()]).is_err());
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::run() {
        eprintln!("LiSnap overlay validation failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("LiSnap Wayland overlay validation is only available on Linux.");
    std::process::exit(2);
}
