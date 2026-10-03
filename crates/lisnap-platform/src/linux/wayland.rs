//! Wayland registry probing for direct capture capabilities; no Portal fallback.
mod ext;
mod wlr;

use std::{collections::BTreeMap, os::unix::net::UnixStream, path::Path};

use lisnap_core::{
    BackendDescriptor, BackendStatus, CaptureBackend, CaptureError, CaptureFrame, CaptureRequest, PixelSize, ScreenInfo,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{wl_output, wl_registry},
};
use wayland_protocols::ext::{
    image_capture_source::v1::client::ext_output_image_capture_source_manager_v1,
    image_copy_capture::v1::client::ext_image_copy_capture_manager_v1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1;

const WL_OUTPUT_MAX_VERSION: u32 = 4;
const WLR_SCREENCOPY_MANAGER_MAX_VERSION: u32 = 3;
const EXT_IMAGE_COPY_CAPTURE_MANAGER_MAX_VERSION: u32 = 1;
const EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER_MAX_VERSION: u32 = 1;
const EXT_IMAGE_COPY_CAPTURE_MANAGER: &str = "ext_image_copy_capture_manager_v1";
const EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER: &str = "ext_output_image_capture_source_manager_v1";
const WLR_SCREENCOPY_MANAGER: &str = "zwlr_screencopy_manager_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaylandCaptureProtocol {
    ExtImageCopyCapture,
    WlrScreencopy,
}

impl std::fmt::Display for WaylandCaptureProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ExtImageCopyCapture => "ext-image-copy-capture",
            Self::WlrScreencopy => "wlr-screencopy",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaylandOutput {
    pub global_name: u32,
    pub protocol_version: u32,
    pub name: Option<String>,
    pub description: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub scale_factor: Option<i32>,
    pub pixel_size: Option<PixelSize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaylandExtConstraints {
    pub output_id: String,
    pub pixel_size: PixelSize,
    pub shm_formats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaylandProbe {
    pub outputs: Vec<WaylandOutput>,
    pub ext_image_copy_capture_manager_version: Option<u32>,
    pub ext_output_image_capture_source_manager_version: Option<u32>,
    pub wlr_screencopy_manager_version: Option<u32>,
    pub ext_image_copy_capture_manager_bound: bool,
    pub ext_output_image_capture_source_manager_bound: bool,
    pub wlr_screencopy_manager_bound: bool,
}

impl WaylandProbe {
    pub fn preferred_capture_protocol(&self) -> Option<WaylandCaptureProtocol> {
        select_capture_protocol(
            self.ext_image_copy_capture_manager_bound,
            self.ext_output_image_capture_source_manager_bound,
            self.wlr_screencopy_manager_bound,
        )
    }
}

fn select_capture_protocol(
    ext_capture_bound: bool,
    ext_source_bound: bool,
    wlr_bound: bool,
) -> Option<WaylandCaptureProtocol> {
    if ext_capture_bound && ext_source_bound {
        Some(WaylandCaptureProtocol::ExtImageCopyCapture)
    } else if wlr_bound {
        Some(WaylandCaptureProtocol::WlrScreencopy)
    } else {
        None
    }
}

#[derive(Debug)]
struct OutputRecord {
    _proxy: wl_output::WlOutput,
    info: WaylandOutput,
    transform: wl_output::Transform,
    mode_size: Option<PixelSize>,
}

#[derive(Debug, Default)]
struct ProbeState {
    outputs: BTreeMap<u32, OutputRecord>,
    ext_image_copy_capture_manager_version: Option<u32>,
    ext_output_image_capture_source_manager_version: Option<u32>,
    wlr_screencopy_manager_version: Option<u32>,
    ext_image_copy_capture_manager: Option<ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1>,
    ext_output_image_capture_source_manager:
        Option<ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1>,
    wlr_screencopy_manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
}

impl ProbeState {
    fn record_global(&mut self, interface: &str, version: u32) {
        let slot = match interface {
            EXT_IMAGE_COPY_CAPTURE_MANAGER => &mut self.ext_image_copy_capture_manager_version,
            EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER => &mut self.ext_output_image_capture_source_manager_version,
            WLR_SCREENCOPY_MANAGER => &mut self.wlr_screencopy_manager_version,
            _ => return,
        };
        *slot = Some(slot.map_or(version, |current| current.max(version)));
    }

    fn finish(self) -> WaylandProbe {
        WaylandProbe {
            outputs: self.outputs.into_values().map(|record| record.info).collect(),
            ext_image_copy_capture_manager_version: self.ext_image_copy_capture_manager_version,
            ext_output_image_capture_source_manager_version: self.ext_output_image_capture_source_manager_version,
            wlr_screencopy_manager_version: self.wlr_screencopy_manager_version,
            ext_image_copy_capture_manager_bound: self.ext_image_copy_capture_manager.is_some(),
            ext_output_image_capture_source_manager_bound: self.ext_output_image_capture_source_manager.is_some(),
            wlr_screencopy_manager_bound: self.wlr_screencopy_manager.is_some(),
        }
    }
}

#[derive(Debug)]
struct OutputData {
    global_name: u32,
}

impl Dispatch<wl_registry::WlRegistry, ()> for ProbeState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } => {
                state.record_global(&interface, version);
                if interface == EXT_IMAGE_COPY_CAPTURE_MANAGER {
                    let proxy = registry.bind::<ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1, _, _>(
                        name,
                        version.min(EXT_IMAGE_COPY_CAPTURE_MANAGER_MAX_VERSION),
                        queue_handle,
                        (),
                    );
                    state.ext_image_copy_capture_manager = Some(proxy);
                }
                if interface == EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER {
                    let proxy = registry
                        .bind::<ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1, _, _>(
                        name,
                        version.min(EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER_MAX_VERSION),
                        queue_handle,
                        (),
                    );
                    state.ext_output_image_capture_source_manager = Some(proxy);
                }
                if interface == WLR_SCREENCOPY_MANAGER {
                    let proxy = registry.bind::<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, _>(
                        name,
                        version.min(WLR_SCREENCOPY_MANAGER_MAX_VERSION),
                        queue_handle,
                        (),
                    );
                    state.wlr_screencopy_manager = Some(proxy);
                }
                if interface == "wl_output" {
                    let bind_version = version.min(WL_OUTPUT_MAX_VERSION);
                    let proxy = registry.bind::<wl_output::WlOutput, _, _>(
                        name,
                        bind_version,
                        queue_handle,
                        OutputData { global_name: name },
                    );
                    state.outputs.insert(
                        name,
                        OutputRecord {
                            _proxy: proxy,
                            info: WaylandOutput {
                                global_name: name,
                                protocol_version: version,
                                name: None,
                                description: None,
                                make: None,
                                model: None,
                                scale_factor: None,
                                pixel_size: None,
                            },
                            transform: wl_output::Transform::Normal,
                            mode_size: None,
                        },
                    );
                }
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.outputs.remove(&name);
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1, ()> for ProbeState {
    fn event(
        _state: &mut Self,
        _manager: &ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
        _event: ext_image_copy_capture_manager_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1, ()> for ProbeState {
    fn event(
        _state: &mut Self,
        _manager: &ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
        _event: ext_output_image_capture_source_manager_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for ProbeState {
    fn event(
        _state: &mut Self,
        _manager: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _event: zwlr_screencopy_manager_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, OutputData> for ProbeState {
    fn event(
        state: &mut Self,
        _output: &wl_output::WlOutput,
        event: wl_output::Event,
        data: &OutputData,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        let Some(record) = state.outputs.get_mut(&data.global_name) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { make, model, transform, .. } => {
                record.info.make = Some(make);
                record.info.model = Some(model);
                if let WEnum::Value(transform) = transform {
                    record.transform = transform;
                    record.info.pixel_size = normalized_pixel_size(record.mode_size, record.transform);
                }
            }
            wl_output::Event::Mode { flags, width, height, .. } => {
                if matches!(flags, WEnum::Value(flags) if flags.contains(wl_output::Mode::Current))
                    && width > 0
                    && height > 0
                {
                    record.mode_size = Some(PixelSize { width: width as u32, height: height as u32 });
                    record.info.pixel_size = normalized_pixel_size(record.mode_size, record.transform);
                }
            }
            wl_output::Event::Scale { factor } => {
                record.info.scale_factor = Some(factor);
            }
            wl_output::Event::Name { name } => {
                record.info.name = Some(name);
            }
            wl_output::Event::Description { description } => {
                record.info.description = Some(description);
            }
            _ => {}
        }
    }
}

pub fn probe() -> Result<WaylandProbe, CaptureError> {
    let connection = Connection::connect_to_env()
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland connection failed: {error}")))?;
    probe_connection(connection)
}

pub fn probe_socket(path: &Path) -> Result<WaylandProbe, CaptureError> {
    let connection = connect_socket(path)?;
    probe_connection(connection)
}

pub fn negotiate_ext_constraints(request: &CaptureRequest) -> Result<Vec<WaylandExtConstraints>, CaptureError> {
    let connection = Connection::connect_to_env()
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland connection failed: {error}")))?;
    map_ext_constraints(ext::negotiate_connection(connection, request)?)
}

pub fn negotiate_ext_constraints_socket(
    path: &Path,
    request: &CaptureRequest,
) -> Result<Vec<WaylandExtConstraints>, CaptureError> {
    map_ext_constraints(ext::negotiate_connection(connect_socket(path)?, request)?)
}

fn map_ext_constraints(
    constraints: Vec<ext::ExtCaptureConstraints>,
) -> Result<Vec<WaylandExtConstraints>, CaptureError> {
    Ok(constraints
        .into_iter()
        .map(|constraint| WaylandExtConstraints {
            output_id: constraint.output_id,
            pixel_size: constraint.pixel_size,
            shm_formats: constraint.shm_formats.into_iter().map(|format| format!("{format:?}")).collect(),
        })
        .collect())
}

pub fn capture_auto(request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError> {
    let probe = probe()?;
    capture_with_probe(request, &probe, None)
}

pub fn capture_auto_socket(path: &Path, request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError> {
    let probe = probe_socket(path)?;
    capture_with_probe(request, &probe, Some(path))
}

fn capture_with_probe(
    request: &CaptureRequest,
    probe: &WaylandProbe,
    socket: Option<&Path>,
) -> Result<Vec<CaptureFrame>, CaptureError> {
    let ext_capture = || match socket {
        Some(path) => capture_ext_socket(path, request),
        None => capture_ext(request),
    };
    let wlr_capture = || match socket {
        Some(path) => capture_wlr_socket(path, request),
        None => capture_wlr(request),
    };
    match probe.preferred_capture_protocol() {
        Some(WaylandCaptureProtocol::ExtImageCopyCapture) => match ext_capture() {
            Ok(frames) => Ok(frames),
            Err(CaptureError::Unsupported(_) | CaptureError::InvalidFrame(_)) if probe.wlr_screencopy_manager_bound => {
                wlr_capture()
            }
            Err(error) => Err(error),
        },
        Some(WaylandCaptureProtocol::WlrScreencopy) => wlr_capture(),
        None => Err(CaptureError::Unsupported(
            "Wayland compositor advertises neither ext-image-copy-capture nor wlr-screencopy".into(),
        )),
    }
}

pub fn capture_ext(request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError> {
    let connection = Connection::connect_to_env()
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland connection failed: {error}")))?;
    ext::capture_connection(connection, request)
}

pub fn capture_ext_socket(path: &Path, request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError> {
    ext::capture_connection(connect_socket(path)?, request)
}

pub fn capture_wlr(request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError> {
    let connection = Connection::connect_to_env()
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland connection failed: {error}")))?;
    wlr::capture_connection(connection, request)
}

pub fn capture_wlr_socket(path: &Path, request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError> {
    wlr::capture_connection(connect_socket(path)?, request)
}

fn connect_socket(path: &Path) -> Result<Connection, CaptureError> {
    let socket = UnixStream::connect(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            CaptureError::PermissionDenied(format!("Wayland socket {}: {error}", path.display()))
        } else {
            CaptureError::BackendFailure(format!("Wayland socket {} failed: {error}", path.display()))
        }
    })?;
    Connection::from_socket(socket)
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland socket handshake failed: {error}")))
}

fn probe_connection(connection: Connection) -> Result<WaylandProbe, CaptureError> {
    let display = connection.display();
    let mut event_queue = connection.new_event_queue::<ProbeState>();
    let queue_handle = event_queue.handle();
    let _registry = display.get_registry(&queue_handle, ());
    let mut state = ProbeState::default();

    event_queue
        .roundtrip(&mut state)
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland registry roundtrip failed: {error}")))?;
    event_queue
        .roundtrip(&mut state)
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland output roundtrip failed: {error}")))?;

    Ok(state.finish())
}

fn normalized_pixel_size(size: Option<PixelSize>, transform: wl_output::Transform) -> Option<PixelSize> {
    let size = size?;
    if matches!(
        transform,
        wl_output::Transform::_90
            | wl_output::Transform::_270
            | wl_output::Transform::Flipped90
            | wl_output::Transform::Flipped270
    ) {
        Some(PixelSize { width: size.height, height: size.width })
    } else {
        Some(size)
    }
}

struct WaylandBackend;

impl CaptureBackend for WaylandBackend {
    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor {
            id: "wayland",
            name: "Wayland native",
            technology: "ext-image-copy-capture / wlr-screencopy SHM",
            status: BackendStatus::Available,
        }
    }

    fn screens(&mut self) -> Result<Vec<ScreenInfo>, CaptureError> {
        let probe = probe()?;
        let screens = probe
            .outputs
            .into_iter()
            .filter_map(|output| {
                let pixel_size = output.pixel_size?;
                let id = output.name.clone().unwrap_or_else(|| output.global_name.to_string());
                let name = output
                    .description
                    .or(output.name)
                    .or_else(|| match (output.make, output.model) {
                        (Some(make), Some(model)) => Some(format!("{make} {model}")),
                        _ => None,
                    })
                    .unwrap_or_else(|| id.clone());
                Some(ScreenInfo {
                    id,
                    name,
                    logical_geometry: None,
                    pixel_size,
                    scale_factor: output.scale_factor.map(f64::from),
                })
            })
            .collect::<Vec<_>>();
        if screens.is_empty() {
            Err(CaptureError::InvalidFrame("Wayland compositor exposed no output with a current pixel mode".into()))
        } else {
            Ok(screens)
        }
    }

    fn capture(&mut self, request: &CaptureRequest) -> Result<Vec<CaptureFrame>, CaptureError> {
        capture_auto(request)
    }
}

pub(crate) fn create() -> Box<dyn CaptureBackend> {
    Box::new(WaylandBackend)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_ext_capture_only_when_both_ext_managers_are_bound() {
        assert_eq!(select_capture_protocol(true, false, true), Some(WaylandCaptureProtocol::WlrScreencopy));
        assert_eq!(select_capture_protocol(false, true, true), Some(WaylandCaptureProtocol::WlrScreencopy));
        assert_eq!(select_capture_protocol(true, true, true), Some(WaylandCaptureProtocol::ExtImageCopyCapture));
    }

    #[test]
    fn falls_back_to_wlr_screencopy() {
        assert_eq!(select_capture_protocol(false, false, true), Some(WaylandCaptureProtocol::WlrScreencopy));
        assert_eq!(select_capture_protocol(false, false, false), None);
    }

    #[test]
    fn normalizes_rotated_output_pixel_size() {
        let size = PixelSize { width: 1920, height: 1080 };
        assert_eq!(normalized_pixel_size(Some(size), wl_output::Transform::Normal), Some(size));
        assert_eq!(
            normalized_pixel_size(Some(size), wl_output::Transform::_90),
            Some(PixelSize { width: 1080, height: 1920 })
        );
        assert_eq!(
            normalized_pixel_size(Some(size), wl_output::Transform::Flipped270),
            Some(PixelSize { width: 1080, height: 1920 })
        );
    }

    #[test]
    fn wayland_backend_reports_available_implementation() {
        assert_eq!(create().descriptor().status, BackendStatus::Available);
    }
}
