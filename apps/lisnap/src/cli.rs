use lisnap_platform::BackendKind;

pub const HELP: &str = "LiSnap - cross-platform screenshot tool (framework stage)\n\nUsage: lisnap [--backend auto|wayland|x11|windows|macos]\n       lisnap --list-backends\n       lisnap --help\n       lisnap --version\n\nWithout options, opens the desktop shell. Native capture is not implemented yet.\n--list-backends does not open a window or request screen access.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    Gui,
    ListBackends,
    Help,
    Version,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cli {
    pub action: Action,
    pub backend: Option<BackendKind>,
}

impl Cli {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut cli = Self::default();
        let mut args = args.into_iter();
        let mut action_seen = false;
        let mut backend_seen = false;
        while let Some(arg) = args.next() {
            let action = match arg.as_str() {
                "-h" | "--help" => Some(Action::Help),
                "-V" | "--version" => Some(Action::Version),
                "--list-backends" => Some(Action::ListBackends),
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
                    return Err("choose only one of --help, --version, or --list-backends".into());
                }
                cli.action = action;
                action_seen = true;
            }
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
    }
}
