//! Wayland registry probing for direct capture capabilities; no Portal fallback.
use std::{collections::BTreeMap, os::unix::net::UnixStream, path::Path};

use lisnap_core::{CaptureBackend, CaptureError};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::{wl_output, wl_registry},
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1;

use crate::{BackendKind, stub::StubBackend};

const WL_OUTPUT_MAX_VERSION: u32 = 4;
const WLR_SCREENCOPY_MANAGER_MAX_VERSION: u32 = 3;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaylandProbe {
    pub outputs: Vec<WaylandOutput>,
    pub ext_image_copy_capture_manager_version: Option<u32>,
    pub ext_output_image_capture_source_manager_version: Option<u32>,
    pub wlr_screencopy_manager_version: Option<u32>,
    pub wlr_screencopy_manager_bound: bool,
}

impl WaylandProbe {
    pub fn preferred_capture_protocol(&self) -> Option<WaylandCaptureProtocol> {
        if self.ext_image_copy_capture_manager_version.is_some()
            && self.ext_output_image_capture_source_manager_version.is_some()
        {
            Some(WaylandCaptureProtocol::ExtImageCopyCapture)
        } else if self.wlr_screencopy_manager_version.is_some() {
            Some(WaylandCaptureProtocol::WlrScreencopy)
        } else {
            None
        }
    }
}

#[derive(Debug)]
struct OutputRecord {
    _proxy: wl_output::WlOutput,
    info: WaylandOutput,
}

#[derive(Debug, Default)]
struct ProbeState {
    outputs: BTreeMap<u32, OutputRecord>,
    ext_image_copy_capture_manager_version: Option<u32>,
    ext_output_image_capture_source_manager_version: Option<u32>,
    wlr_screencopy_manager_version: Option<u32>,
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
                            },
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
            wl_output::Event::Geometry { make, model, .. } => {
                record.info.make = Some(make);
                record.info.model = Some(model);
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
    let socket = UnixStream::connect(path)
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland socket {} failed: {error}", path.display())))?;
    let connection = Connection::from_socket(socket)
        .map_err(|error| CaptureError::BackendFailure(format!("Wayland socket handshake failed: {error}")))?;
    probe_connection(connection)
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

pub(crate) fn create() -> Box<dyn CaptureBackend> {
    Box::new(StubBackend(BackendKind::Wayland))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_ext_capture_only_when_both_ext_globals_exist() {
        let mut state = ProbeState::default();
        state.record_global(EXT_IMAGE_COPY_CAPTURE_MANAGER, 1);
        assert_eq!(state.finish().preferred_capture_protocol(), None);

        let mut state = ProbeState::default();
        state.record_global(EXT_IMAGE_COPY_CAPTURE_MANAGER, 1);
        state.record_global(EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER, 1);
        assert_eq!(state.finish().preferred_capture_protocol(), Some(WaylandCaptureProtocol::ExtImageCopyCapture));
    }

    #[test]
    fn falls_back_to_wlr_screencopy() {
        let mut state = ProbeState::default();
        state.record_global(WLR_SCREENCOPY_MANAGER, 3);
        let probe = state.finish();
        assert_eq!(probe.preferred_capture_protocol(), Some(WaylandCaptureProtocol::WlrScreencopy));
        assert_eq!(probe.wlr_screencopy_manager_version, Some(3));
        assert!(!probe.wlr_screencopy_manager_bound);
    }
}
