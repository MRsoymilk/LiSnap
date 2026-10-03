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
        Action::ProbeWayland => print_wayland_probe(cli.wayland_socket.as_deref()),
        Action::ProbeExtConstraints => run_ext_constraint_probe(&cli),
        Action::CaptureWayland => run_wayland_capture(&cli),
        Action::CaptureWlr => run_wlr_capture(&cli),
        Action::Gui => launch_gui(cli.backend.or_else(default_backend)),
    }
}

#[cfg(target_os = "linux")]
fn print_wayland_probe(socket: Option<&str>) -> Result<(), String> {
    let probe = match socket {
        Some(path) => lisnap_platform::probe_wayland_socket(std::path::Path::new(path)),
        None => lisnap_platform::probe_wayland(),
    }
    .map_err(|error| error.to_string())?;
    println!("Wayland outputs: {}", probe.outputs.len());
    for output in &probe.outputs {
        println!(
            "output {} | {} | wl_output v{} | pixels {} | scale {}",
            output.global_name,
            output.name.as_deref().unwrap_or("<unnamed>"),
            output.protocol_version,
            output.pixel_size.map_or_else(|| "unknown".into(), |size| format!("{}x{}", size.width, size.height)),
            output.scale_factor.map_or_else(|| "unknown".into(), |scale| scale.to_string())
        );
        if let Some(description) = output.description.as_deref() {
            println!("  description: {description}");
        }
        if output.make.is_some() || output.model.is_some() {
            println!(
                "  device: {} {}",
                output.make.as_deref().unwrap_or("<unknown make>"),
                output.model.as_deref().unwrap_or("<unknown model>")
            );
        }
    }
    println!("ext-image-copy-capture manager: {}", protocol_version(probe.ext_image_copy_capture_manager_version));
    println!(
        "ext-output-image-capture-source manager: {}",
        protocol_version(probe.ext_output_image_capture_source_manager_version)
    );
    println!("ext-image-copy-capture manager bound: {}", probe.ext_image_copy_capture_manager_bound);
    println!("ext-output-image-capture-source manager bound: {}", probe.ext_output_image_capture_source_manager_bound);
    println!("wlr-screencopy manager: {}", protocol_version(probe.wlr_screencopy_manager_version));
    println!("wlr-screencopy manager bound: {}", probe.wlr_screencopy_manager_bound);
    println!(
        "preferred direct capture protocol: {}",
        probe.preferred_capture_protocol().map_or_else(|| "none".into(), |protocol| protocol.to_string())
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn print_wayland_probe(_socket: Option<&str>) -> Result<(), String> {
    Err("--probe-wayland is only available on Linux".into())
}

#[cfg(target_os = "linux")]
fn run_ext_constraint_probe(cli: &Cli) -> Result<(), String> {
    let request =
        lisnap_core::CaptureRequest { screen_id: cli.screen_id.clone(), include_cursor: false, cancellation: None };
    let constraints = match cli.wayland_socket.as_deref() {
        Some(path) => lisnap_platform::negotiate_wayland_ext_constraints_socket(std::path::Path::new(path), &request),
        None => lisnap_platform::negotiate_wayland_ext_constraints(&request),
    }
    .map_err(|error| error.to_string())?;
    if constraints.is_empty() {
        return Err("ext-image-copy-capture returned no output constraints".into());
    }
    for constraint in constraints {
        println!(
            "{} | {}x{} | SHM {}",
            constraint.output_id,
            constraint.pixel_size.width,
            constraint.pixel_size.height,
            constraint.shm_formats.join(",")
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_ext_constraint_probe(_cli: &Cli) -> Result<(), String> {
    Err("--probe-ext-constraints is only available on Linux".into())
}

#[cfg(target_os = "linux")]
fn run_wayland_capture(cli: &Cli) -> Result<(), String> {
    let request = lisnap_core::CaptureRequest {
        screen_id: cli.screen_id.clone(),
        include_cursor: cli.include_cursor,
        cancellation: None,
    };
    let probe = match cli.wayland_socket.as_deref() {
        Some(path) => lisnap_platform::probe_wayland_socket(std::path::Path::new(path)),
        None => lisnap_platform::probe_wayland(),
    }
    .map_err(|error| error.to_string())?;
    println!(
        "Wayland capture protocol: {}",
        probe.preferred_capture_protocol().map_or_else(|| "none".into(), |protocol| protocol.to_string())
    );
    let frames = match cli.wayland_socket.as_deref() {
        Some(path) => lisnap_platform::capture_wayland_socket(std::path::Path::new(path), &request),
        None => lisnap_platform::capture_wayland(&request),
    }
    .map_err(|error| error.to_string())?;
    print_capture_frames("Wayland capture", frames)
}

#[cfg(not(target_os = "linux"))]
fn run_wayland_capture(_cli: &Cli) -> Result<(), String> {
    Err("--capture-wayland is only available on Linux".into())
}

#[cfg(target_os = "linux")]
fn run_wlr_capture(cli: &Cli) -> Result<(), String> {
    let request = lisnap_core::CaptureRequest {
        screen_id: cli.screen_id.clone(),
        include_cursor: cli.include_cursor,
        cancellation: None,
    };
    let frames = match cli.wayland_socket.as_deref() {
        Some(path) => lisnap_platform::capture_wayland_wlr_socket(std::path::Path::new(path), &request),
        None => lisnap_platform::capture_wayland_wlr(&request),
    }
    .map_err(|error| error.to_string())?;
    print_capture_frames("wlr-screencopy", frames)
}

#[cfg(target_os = "linux")]
fn print_capture_frames(label: &str, frames: Vec<lisnap_core::CaptureFrame>) -> Result<(), String> {
    if frames.is_empty() {
        return Err(format!("{label} returned no frames"));
    }
    for frame in frames {
        let size = frame.image.size();
        let pixels = frame.image.pixels();
        println!(
            "{} | {} | {}x{} | scale {} | {} RGBA bytes | fnv64 {:016x}",
            frame.screen.id,
            frame.screen.name,
            size.width,
            size.height,
            frame.screen.scale_factor.map_or_else(|| "unknown".into(), |scale| scale.to_string()),
            pixels.len(),
            fnv1a64(pixels)
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_wlr_capture(_cli: &Cli) -> Result<(), String> {
    Err("--capture-wlr is only available on Linux".into())
}

#[cfg(target_os = "linux")]
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3))
}

#[cfg(target_os = "linux")]
fn protocol_version(version: Option<u32>) -> String {
    version.map_or_else(|| "unavailable".into(), |version| format!("v{version}"))
}

#[cfg(feature = "gui")]
fn launch_gui(kind: Option<BackendKind>) -> Result<(), String> {
    let selected = kind
        .map(lisnap_platform::create_backend)
        .transpose()
        .map_err(|error| error.to_string())?
        .map(|backend| backend.descriptor());
    let capture_launcher = match (kind, selected) {
        (Some(kind), Some(descriptor)) if descriptor.status == lisnap_core::BackendStatus::Available => {
            Some(make_capture_launcher(kind))
        }
        _ => None,
    };
    let backends = BackendKind::ALL.into_iter().map(BackendKind::descriptor).collect();
    lisnap_ui::run(selected, backends, capture_launcher).map_err(|error| error.to_string())
}

#[cfg(feature = "gui")]
fn make_capture_launcher(kind: BackendKind) -> lisnap_ui::CaptureLauncher {
    std::sync::Arc::new(move |request| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = lisnap_platform::create_backend(kind)
                .and_then(|mut backend| backend.capture(&request))
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        receiver
    })
}

#[cfg(not(feature = "gui"))]
fn launch_gui(_kind: Option<BackendKind>) -> Result<(), String> {
    Err("GUI support is disabled; rebuild with default features, or use --list-backends".into())
}
