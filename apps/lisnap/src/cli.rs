use lisnap_platform::BackendKind;

pub const HELP: &str = "LiSnap - cross-platform screenshot tool (framework stage)\n\nUsage: lisnap [--backend auto|wayland|x11|windows|macos]\n       lisnap --list-backends\n       lisnap --probe-wayland [--wayland-socket PATH]\n       lisnap --capture-wlr [--wayland-socket PATH] [--screen ID] [--include-cursor]\n       lisnap --help\n       lisnap --version\n\nWithout options, opens the desktop shell. Native capture is still under active implementation.\n--list-backends does not open a window or request screen access.\n--probe-wayland reports outputs/capture protocols without capturing pixels.\n--capture-wlr performs direct wlr-screencopy SHM capture and prints frame metadata/checksums without saving files.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    Gui,
    ListBackends,
    ProbeWayland,
    CaptureWlr,
    Help,
    Version,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    pub action: Action,
    pub backend: Option<BackendKind>,
    pub wayland_socket: Option<String>,
    pub screen_id: Option<String>,
    pub include_cursor: bool,
}

impl Cli {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut cli = Self::default();
        let mut args = args.into_iter();
        let mut action_seen = false;
        let mut backend_seen = false;
        let mut wayland_socket_seen = false;
        let mut screen_seen = false;
        while let Some(arg) = args.next() {
            let action = match arg.as_str() {
                "-h" | "--help" => Some(Action::Help),
                "-V" | "--version" => Some(Action::Version),
                "--list-backends" => Some(Action::ListBackends),
                "--probe-wayland" => Some(Action::ProbeWayland),
                "--capture-wlr" => Some(Action::CaptureWlr),
                "--wayland-socket" => {
                    if wayland_socket_seen {
                        return Err("--wayland-socket may only be specified once".into());
                    }
                    cli.wayland_socket = Some(args.next().ok_or("--wayland-socket requires a path")?);
                    wayland_socket_seen = true;
                    None
                }
                "--screen" => {
                    if screen_seen {
                        return Err("--screen may only be specified once".into());
                    }
                    cli.screen_id = Some(args.next().ok_or("--screen requires an output ID")?);
                    screen_seen = true;
                    None
                }
                "--include-cursor" => {
                    if cli.include_cursor {
                        return Err("--include-cursor may only be specified once".into());
                    }
                    cli.include_cursor = true;
                    None
                }
                "--backend" => {
                    if backend_seen {
                        return Err("--backend may only be specified once".into());
                    }
                    let value = args.next().ok_or("--backend requires a value")?;
                    cli.backend = if value == "auto" { None } else { Some(value.parse()?) };
                    backend_seen = true;
                    None
                }
                _ => return Err(format!("unknown argument '{arg}'; use --help")),
            };
            if let Some(action) = action {
                if action_seen {
                    return Err(
                        "choose only one of --help, --version, --list-backends, --probe-wayland, or --capture-wlr"
                            .into(),
                    );
                }
                cli.action = action;
                action_seen = true;
            }
        }
        if cli.wayland_socket.is_some() && !matches!(cli.action, Action::ProbeWayland | Action::CaptureWlr) {
            return Err("--wayland-socket requires --probe-wayland or --capture-wlr".into());
        }
        if (cli.screen_id.is_some() || cli.include_cursor) && cli.action != Action::CaptureWlr {
            return Err("--screen and --include-cursor require --capture-wlr".into());
        }
        Ok(cli)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Cli, String> {
        Cli::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn defaults_to_gui_with_automatic_backend() {
        assert_eq!(parse(&[]).unwrap(), Cli::default());
    }

    #[test]
    fn parses_explicit_backend_and_headless_listing() {
        assert_eq!(parse(&["--backend", "wayland"]).unwrap().backend, Some(BackendKind::Wayland));
        assert_eq!(parse(&["--backend", "auto"]).unwrap().backend, None);
        assert_eq!(parse(&["--list-backends"]).unwrap().action, Action::ListBackends);
        assert_eq!(parse(&["--probe-wayland"]).unwrap().action, Action::ProbeWayland);
        let capture = parse(&["--capture-wlr", "--screen", "DP-1", "--include-cursor"]).unwrap();
        assert_eq!(capture.action, Action::CaptureWlr);
        assert_eq!(capture.screen_id.as_deref(), Some("DP-1"));
        assert!(capture.include_cursor);
        assert_eq!(
            parse(&["--probe-wayland", "--wayland-socket", "/run/user/1000/wayland-1"])
                .unwrap()
                .wayland_socket
                .as_deref(),
            Some("/run/user/1000/wayland-1")
        );
    }

    #[test]
    fn rejects_missing_unknown_and_duplicate_arguments() {
        for args in
            [&["--backend"][..], &["--backend", "portal"], &["--unknown"], &["--backend", "x11", "--backend", "auto"]]
        {
            assert!(parse(args).is_err());
        }
    }

    #[test]
    fn rejects_conflicting_actions() {
        assert!(parse(&["--list-backends", "--version"]).is_err());
        assert!(parse(&["--probe-wayland", "--help"]).is_err());
        assert!(parse(&["--capture-wlr", "--version"]).is_err());
        assert!(parse(&["--wayland-socket", "/run/user/1000/wayland-1"]).is_err());
        assert!(parse(&["--screen", "DP-1"]).is_err());
        assert!(parse(&["--include-cursor"]).is_err());
    }
}
