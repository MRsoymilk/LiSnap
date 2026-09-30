mod cli;

use cli::{Action, Cli};
use lisnap_platform::{BackendKind, default_backend};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = std::env::args_os()
        .skip(1)
        .map(|value| value.into_string().map_err(|_| "arguments must be valid Unicode".to_owned()))
        .collect::<Result<Vec<_>, _>>();
    let cli = match args.and_then(Cli::parse) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("LiSnap: {error}");
            return ExitCode::from(2);
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("LiSnap: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.action {
        Action::Help => {
            print!("{}", cli::HELP);
            Ok(())
        }
        Action::Version => {
            println!("LiSnap {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Action::ListBackends => {
            println!("LiSnap {} - native capture backend inventory", env!("CARGO_PKG_VERSION"));
            println!(
                "Session candidate: {} (environment hint only)",
                default_backend().map_or_else(|| "none".into(), |kind| kind.to_string())
            );
            for kind in BackendKind::ALL {
                let backend = kind.descriptor();
                println!("{} | {} | {}", backend.id, backend.status, backend.technology);
            }
            Ok(())
        }
        Action::Gui => launch_gui(cli.backend.or_else(default_backend)),
    }
}

#[cfg(feature = "gui")]
fn launch_gui(kind: Option<BackendKind>) -> Result<(), String> {
    let selected = kind
        .map(lisnap_platform::create_backend)
        .transpose()
        .map_err(|error| error.to_string())?
        .map(|backend| backend.descriptor());
    let backends = BackendKind::ALL.into_iter().map(BackendKind::descriptor).collect();
    lisnap_ui::run(selected, backends).map_err(|error| error.to_string())
}

#[cfg(not(feature = "gui"))]
fn launch_gui(_kind: Option<BackendKind>) -> Result<(), String> {
    Err("GUI support is disabled; rebuild with default features, or use --list-backends".into())
}
