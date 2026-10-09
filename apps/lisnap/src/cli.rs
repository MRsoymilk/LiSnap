use lisnap_platform::BackendKind;

pub const HELP: &str = "LiSnap - cross-platform screenshot tool\n\nUsage: lisnap [--backend auto|wayland] [--screen ID] [--output PATH] [--include-cursor|--no-cursor]\n       lisnap --settings\n       lisnap --window [--backend auto|wayland|x11|windows|macos]\n       lisnap --list-backends\n       lisnap --list-screens [--backend auto|wayland|x11|windows|macos]\n       lisnap --probe-wayland [--wayland-socket PATH]\n       lisnap --probe-ext-constraints [--wayland-socket PATH] [--screen ID]\n       lisnap --capture-wayland [--wayland-socket PATH] [--screen ID] [--include-cursor|--no-cursor]\n       lisnap --capture-wlr [--wayland-socket PATH] [--screen ID] [--include-cursor|--no-cursor]\n       lisnap --help\n       lisnap --version\n\nWithout an action, starts the interactive screenshot editor directly. On Hyprland, the active monitor is used automatically.\n--settings (or 'lisnap settings') opens the graphical preferences window.\n--include-cursor and --no-cursor override the saved mouse pointer preference for one capture.\n--window opens the development/diagnostic desktop shell.\n--output writes the accepted annotated crop to the requested PNG path; otherwise LiSnap writes under ~/Pictures/LiSnap.\n--list-backends does not open a window or request screen access.\n--list-screens uses the selected native backend to enumerate stable output IDs without capturing pixels.\n--probe-wayland reports outputs/capture protocols without capturing pixels.\n--probe-ext-constraints creates an ext-image-copy-capture session and reports SHM constraints without capturing pixels.\n--capture-wayland uses ext-image-copy-capture when available and falls back to wlr-screencopy, then prints frame metadata/checksums.\n--capture-wlr forces the wlr-screencopy SHM path for diagnostics.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    InteractiveCapture,
    Window,
    Settings,
    ListBackends,
    ListScreens,
    ProbeWayland,
    ProbeExtConstraints,
    CaptureWayland,
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
    pub output_path: Option<String>,
    /// None uses the saved preference; Some temporarily overrides it.
    pub include_cursor: Option<bool>,
}

impl Cli {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut cli = Self::default();
        let mut args = args.into_iter();
        let mut action_seen = false;
        let mut backend_seen = false;
        let mut wayland_socket_seen = false;
        let mut screen_seen = false;
        let mut output_seen = false;
        let mut cursor_seen = false;
        while let Some(arg) = args.next() {
            let action = match arg.as_str() {
                "-h" | "--help" => Some(Action::Help),
                "-V" | "--version" => Some(Action::Version),
                "--window" => Some(Action::Window),
                "--settings" | "settings" => Some(Action::Settings),
                "--list-backends" => Some(Action::ListBackends),
                "--list-screens" => Some(Action::ListScreens),
                "--probe-wayland" => Some(Action::ProbeWayland),
                "--probe-ext-constraints" => Some(Action::ProbeExtConstraints),
                "--capture-wayland" => Some(Action::CaptureWayland),
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
                "--output" => {
                    if output_seen {
                        return Err("--output may only be specified once".into());
                    }
                    cli.output_path = Some(args.next().ok_or("--output requires a path")?);
                    output_seen = true;
                    None
                }
                "--include-cursor" | "--no-cursor" => {
                    if cursor_seen {
                        return Err("choose only one of --include-cursor or --no-cursor".into());
                    }
                    cli.include_cursor = Some(arg == "--include-cursor");
                    cursor_seen = true;
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
                        "choose only one of --settings, --window, --help, --version, --list-backends, --list-screens, --probe-wayland, --probe-ext-constraints, --capture-wayland, or --capture-wlr"
                            .into(),
                    );
                }
                cli.action = action;
                action_seen = true;
            }
        }
        if cli.wayland_socket.is_some()
            && !matches!(
                cli.action,
                Action::InteractiveCapture
                    | Action::ProbeWayland
                    | Action::ProbeExtConstraints
                    | Action::CaptureWayland
                    | Action::CaptureWlr
            )
        {
            return Err(
                "--wayland-socket is only valid for interactive Wayland capture or Wayland diagnostic actions".into()
            );
        }
        if cli.screen_id.is_some()
            && !matches!(
                cli.action,
                Action::InteractiveCapture | Action::ProbeExtConstraints | Action::CaptureWayland | Action::CaptureWlr
            )
        {
            return Err("--screen is only valid for interactive capture or Wayland capture diagnostics".into());
        }
        if cli.output_path.is_some() && cli.action != Action::InteractiveCapture {
            return Err("--output is only valid for interactive capture".into());
        }
        if cli.include_cursor.is_some()
            && !matches!(cli.action, Action::InteractiveCapture | Action::CaptureWayland | Action::CaptureWlr)
        {
            return Err("cursor overrides are only valid for interactive capture or Wayland capture diagnostics".into());
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
    fn defaults_to_interactive_capture_with_automatic_backend() {
        assert_eq!(parse(&[]).unwrap(), Cli::default());
        assert_eq!(parse(&[]).unwrap().action, Action::InteractiveCapture);
    }

    #[test]
    fn parses_explicit_backend_and_headless_listing() {
        assert_eq!(parse(&["--backend", "wayland"]).unwrap().backend, Some(BackendKind::Wayland));
        assert_eq!(parse(&["--backend", "auto"]).unwrap().backend, None);
        assert_eq!(parse(&["--window"]).unwrap().action, Action::Window);
        assert_eq!(parse(&["--settings"]).unwrap().action, Action::Settings);
        assert_eq!(parse(&["settings"]).unwrap().action, Action::Settings);
        assert_eq!(parse(&["--list-backends"]).unwrap().action, Action::ListBackends);
        assert_eq!(parse(&["--list-screens"]).unwrap().action, Action::ListScreens);
        assert_eq!(parse(&["--list-screens", "--backend", "wayland"]).unwrap().backend, Some(BackendKind::Wayland));
        assert_eq!(parse(&["--probe-wayland"]).unwrap().action, Action::ProbeWayland);
        let ext = parse(&["--probe-ext-constraints", "--screen", "DP-1"]).unwrap();
        assert_eq!(ext.action, Action::ProbeExtConstraints);
        assert_eq!(ext.screen_id.as_deref(), Some("DP-1"));
        let automatic = parse(&["--capture-wayland", "--screen", "DP-1", "--include-cursor"]).unwrap();
        assert_eq!(automatic.action, Action::CaptureWayland);
        assert_eq!(automatic.screen_id.as_deref(), Some("DP-1"));
        assert_eq!(automatic.include_cursor, Some(true));
        let capture = parse(&["--capture-wlr", "--screen", "DP-1", "--include-cursor"]).unwrap();
        assert_eq!(capture.action, Action::CaptureWlr);
        assert_eq!(capture.screen_id.as_deref(), Some("DP-1"));
        assert_eq!(capture.include_cursor, Some(true));
        let interactive = parse(&["--screen", "DP-1", "--output", "/tmp/lisnap.png", "--include-cursor"]).unwrap();
        assert_eq!(interactive.action, Action::InteractiveCapture);
        assert_eq!(interactive.screen_id.as_deref(), Some("DP-1"));
        assert_eq!(interactive.output_path.as_deref(), Some("/tmp/lisnap.png"));
        assert_eq!(interactive.include_cursor, Some(true));
        assert_eq!(
            parse(&["--probe-wayland", "--wayland-socket", "/run/user/1000/wayland-1"])
                .unwrap()
                .wayland_socket
                .as_deref(),
            Some("/run/user/1000/wayland-1")
        );
    }

    #[test]
    fn parses_cursor_overrides() {
        assert_eq!(parse(&[]).unwrap().include_cursor, None);
        assert_eq!(parse(&["--include-cursor"]).unwrap().include_cursor, Some(true));
        assert_eq!(parse(&["--no-cursor"]).unwrap().include_cursor, Some(false));
        assert_eq!(parse(&["--capture-wlr", "--no-cursor"]).unwrap().include_cursor, Some(false));
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
        assert!(parse(&["--list-screens", "--help"]).is_err());
        assert!(parse(&["--probe-wayland", "--help"]).is_err());
        assert!(parse(&["--capture-wlr", "--version"]).is_err());
        assert!(parse(&["--capture-wayland", "--version"]).is_err());
        assert!(parse(&["--probe-ext-constraints", "--help"]).is_err());
        assert!(parse(&["--window", "--screen", "DP-1"]).is_err());
        assert!(parse(&["--settings", "--window"]).is_err());
        assert!(parse(&["--settings", "--include-cursor"]).is_err());
        assert!(parse(&["--include-cursor", "--no-cursor"]).is_err());
        assert!(parse(&["--no-cursor", "--no-cursor"]).is_err());
        assert!(parse(&["--list-backends", "--output", "/tmp/a.png"]).is_err());
    }
}
