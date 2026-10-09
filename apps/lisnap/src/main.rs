mod cli;

use cli::{Action, Cli};
use lisnap_core::AppSettings;
use lisnap_platform::{BackendKind, default_backend};
use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{SystemTime, UNIX_EPOCH},
};

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
        Action::ListScreens => list_screens(cli.backend.or_else(default_backend)),
        Action::ProbeWayland => print_wayland_probe(cli.wayland_socket.as_deref()),
        Action::ProbeExtConstraints => run_ext_constraint_probe(&cli),
        Action::CaptureWayland => run_wayland_capture(&cli),
        Action::CaptureWlr => run_wlr_capture(&cli),
        Action::InteractiveCapture => run_interactive_capture(&cli),
        Action::Window => launch_gui(cli.backend.or_else(default_backend), AppSettings::load()?.include_cursor),
        Action::Settings => launch_settings(),
    }
}

fn effective_include_cursor(cli: &Cli) -> Result<bool, String> {
    match cli.include_cursor {
        Some(explicit) => Ok(explicit),
        None => Ok(AppSettings::load()?.include_cursor),
    }
}

#[cfg(target_os = "linux")]
fn run_interactive_capture(cli: &Cli) -> Result<(), String> {
    let kind = cli.backend.or_else(default_backend).ok_or("no native capture backend detected")?;
    if kind != BackendKind::Wayland {
        return Err(format!(
            "interactive capture currently requires the Wayland backend; detected {kind}. Use --window for the development shell."
        ));
    }

    let screen_id = resolve_interactive_wayland_screen(cli)?;
    let request = lisnap_core::CaptureRequest {
        screen_id: Some(screen_id.clone()),
        include_cursor: effective_include_cursor(cli)?,
        cancellation: None,
    };
    let frames = match cli.wayland_socket.as_deref() {
        Some(path) => lisnap_platform::capture_wayland_socket(Path::new(path), &request),
        None => lisnap_platform::capture_wayland(&request),
    }
    .map_err(|error| error.to_string())?;
    let frame =
        frames.into_iter().next().ok_or_else(|| format!("Wayland capture returned no frame for output {screen_id}"))?;

    let outcome = match cli.wayland_socket.as_deref() {
        Some(path) => lisnap_platform::run_wayland_editor_socket(Path::new(path), &frame),
        None => lisnap_platform::run_wayland_editor(&frame),
    }
    .map_err(|error| error.to_string())?;

    match outcome {
        lisnap_platform::WaylandEditorOutcome::Accepted { selection, image } => {
            let output = cli.output_path.as_deref().map(PathBuf::from).unwrap_or_else(default_output_path);
            save_png(&output, &image)?;
            println!(
                "Saved {}x{} annotated capture from {} at {},{} to {}",
                selection.width,
                selection.height,
                frame.screen.id,
                selection.x,
                selection.y,
                output.display()
            );
            Ok(())
        }
        lisnap_platform::WaylandEditorOutcome::Cancelled => Ok(()),
        lisnap_platform::WaylandEditorOutcome::Closed => {
            Err("Wayland compositor closed the interactive screenshot editor".into())
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run_interactive_capture(_cli: &Cli) -> Result<(), String> {
    Err("interactive screenshot editing is currently implemented for Linux/Wayland only; use --window".into())
}

#[cfg(target_os = "linux")]
fn resolve_interactive_wayland_screen(cli: &Cli) -> Result<String, String> {
    if let Some(screen) = &cli.screen_id {
        return Ok(screen.clone());
    }
    let probe = match cli.wayland_socket.as_deref() {
        Some(path) => lisnap_platform::probe_wayland_socket(Path::new(path)),
        None => lisnap_platform::probe_wayland(),
    }
    .map_err(|error| error.to_string())?;

    if let Some(active) = hyprland_active_monitor()
        && probe.outputs.iter().any(|output| output.name.as_deref() == Some(active.as_str()))
    {
        return Ok(active);
    }

    if probe.outputs.len() == 1 {
        let output = &probe.outputs[0];
        return Ok(output.name.clone().unwrap_or_else(|| output.global_name.to_string()));
    }

    Err(
        "cannot determine the active monitor in this multi-output Wayland session; on Hyprland ensure hyprctl is available, or pass --screen OUTPUT"
            .into(),
    )
}

#[cfg(target_os = "linux")]
fn hyprland_active_monitor() -> Option<String> {
    let output = Command::new("hyprctl").args(["-j", "activeworkspace"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    json_string_field(text, "monitor")
}

#[cfg(target_os = "linux")]
fn json_string_field(input: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\"");
    let mut rest = input.get(input.find(&marker)? + marker.len()..)?.trim_start();
    rest = rest.strip_prefix(':')?.trim_start();
    rest = rest.strip_prefix('"')?;
    let mut result = String::new();
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            result.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(result);
        } else {
            result.push(ch);
        }
    }
    None
}

fn default_output_path() -> PathBuf {
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Pictures").join("LiSnap"))
        .unwrap_or_else(|| PathBuf::from("."));
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    base.join(format!("LiSnap-{}-{:03}.png", duration.as_secs(), duration.subsec_millis()))
}

fn save_png(path: &Path, image: &lisnap_core::RgbaImage) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("creating screenshot directory {} failed: {error}", parent.display()))?;
    }
    let file = File::create(path).map_err(|error| format!("creating {} failed: {error}", path.display()))?;
    let writer = BufWriter::new(file);
    let size = image.size();
    let mut encoder = png::Encoder::new(writer, size.width, size.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| format!("writing PNG header failed: {error}"))?;
    writer.write_image_data(image.pixels()).map_err(|error| format!("writing PNG pixels failed: {error}"))
}

fn list_screens(kind: Option<BackendKind>) -> Result<(), String> {
    let kind = kind.ok_or("no native backend candidate; specify --backend explicitly")?;
    let mut backend = lisnap_platform::create_backend(kind).map_err(|error| error.to_string())?;
    let screens = backend.screens().map_err(|error| error.to_string())?;
    if screens.is_empty() {
        return Err(format!("{kind} returned no screens"));
    }
    println!("{} screens: {}", kind, screens.len());
    for screen in screens {
        println!(
            "{} | {} | {}x{} | scale {}",
            screen.id,
            screen.name,
            screen.pixel_size.width,
            screen.pixel_size.height,
            screen.scale_factor.map_or_else(|| "unknown".into(), |scale| scale.to_string())
        );
    }
    Ok(())
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
        include_cursor: effective_include_cursor(cli)?,
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
        include_cursor: effective_include_cursor(cli)?,
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
fn launch_settings() -> Result<(), String> {
    let config_path = AppSettings::config_path()?;
    let settings = AppSettings::load_from(&config_path)?;
    lisnap_ui::run_settings(settings, config_path).map_err(|error| error.to_string())
}

#[cfg(not(feature = "gui"))]
fn launch_settings() -> Result<(), String> {
    Err("settings window requires GUI support; rebuild with default features".into())
}

#[cfg(feature = "gui")]
fn launch_gui(kind: Option<BackendKind>, include_cursor: bool) -> Result<(), String> {
    let selected = kind
        .map(lisnap_platform::create_backend)
        .transpose()
        .map_err(|error| error.to_string())?
        .map(|backend| backend.descriptor());
    let (capture_launcher, screen_launcher, overlay_launcher) = match (kind, selected) {
        (Some(kind), Some(descriptor)) if descriptor.status == lisnap_core::BackendStatus::Available => {
            (Some(make_capture_launcher(kind)), Some(make_screen_launcher(kind)), make_overlay_launcher(kind))
        }
        _ => (None, None, None),
    };
    let backends = BackendKind::ALL.into_iter().map(BackendKind::descriptor).collect();
    lisnap_ui::run_with_overlay_and_cursor(
        selected,
        backends,
        capture_launcher,
        screen_launcher,
        overlay_launcher,
        include_cursor,
    )
    .map_err(|error| error.to_string())
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

#[cfg(feature = "gui")]
fn make_screen_launcher(kind: BackendKind) -> lisnap_ui::ScreenLauncher {
    std::sync::Arc::new(move || {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = lisnap_platform::create_backend(kind)
                .and_then(|mut backend| backend.screens())
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        receiver
    })
}

#[cfg(all(feature = "gui", target_os = "linux"))]
fn make_overlay_launcher(kind: BackendKind) -> Option<lisnap_ui::OverlayLauncher> {
    if kind != BackendKind::Wayland {
        return None;
    }
    Some(std::sync::Arc::new(move |frame| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = match lisnap_platform::run_wayland_frozen_overlay(&frame) {
                Ok(lisnap_platform::WaylandOverlayOutcome::Selected(rect)) => Ok(Some(rect)),
                Ok(lisnap_platform::WaylandOverlayOutcome::Cancelled) => Ok(None),
                Ok(lisnap_platform::WaylandOverlayOutcome::Closed) => {
                    Err("Wayland compositor closed the selection overlay".into())
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = sender.send(result);
        });
        receiver
    }))
}

#[cfg(all(feature = "gui", not(target_os = "linux")))]
fn make_overlay_launcher(_kind: BackendKind) -> Option<lisnap_ui::OverlayLauncher> {
    None
}

#[cfg(not(feature = "gui"))]
fn launch_gui(_kind: Option<BackendKind>, _include_cursor: bool) -> Result<(), String> {
    Err("GUI support is disabled; rebuild with default features, or use --list-backends".into())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn command_line_cursor_override_takes_priority_over_persisted_settings() {
        let show = Cli::parse(["--include-cursor".to_owned()]).unwrap();
        let hide = Cli::parse(["--no-cursor".to_owned()]).unwrap();
        assert!(effective_include_cursor(&show).unwrap());
        assert!(!effective_include_cursor(&hide).unwrap());
        let persisted = AppSettings { include_cursor: true };
        assert!(persisted.include_cursor_with_override(None));
        assert!(persisted.include_cursor_with_override(show.include_cursor));
        assert!(!persisted.include_cursor_with_override(hide.include_cursor));
    }

    #[test]
    fn parses_hyprland_active_monitor_name() {
        let json = r#"{"id":3,"name":"3","monitor":"HDMI-A-1","windows":2}"#;
        assert_eq!(json_string_field(json, "monitor").as_deref(), Some("HDMI-A-1"));
    }

    #[test]
    fn rejects_missing_or_non_string_monitor_field() {
        assert_eq!(json_string_field(r#"{"name":"3"}"#, "monitor"), None);
        assert_eq!(json_string_field(r#"{"monitor":3}"#, "monitor"), None);
    }
}
