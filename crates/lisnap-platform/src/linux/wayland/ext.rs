use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use lisnap_core::{CaptureError, CaptureRequest, PixelSize};
use rustix::{
    event::{PollFd, PollFlags, poll},
    time::Timespec,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{wl_callback, wl_display, wl_output, wl_registry, wl_shm},
};
use wayland_protocols::ext::{
    image_capture_source::v1::client::{ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1},
    image_copy_capture::v1::client::{ext_image_copy_capture_manager_v1, ext_image_copy_capture_session_v1},
};

const WL_OUTPUT_MAX_VERSION: u32 = 4;
const EXT_IMAGE_COPY_CAPTURE_MANAGER_MAX_VERSION: u32 = 1;
const EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER_MAX_VERSION: u32 = 1;
const EXT_IMAGE_COPY_CAPTURE_MANAGER: &str = "ext_image_copy_capture_manager_v1";
const EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER: &str = "ext_output_image_capture_source_manager_v1";
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_POLL_SLICE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtCaptureConstraints {
    pub(crate) output_id: String,
    pub(crate) pixel_size: PixelSize,
    pub(crate) shm_formats: Vec<wl_shm::Format>,
}

#[derive(Debug)]
struct OutputRecord {
    proxy: wl_output::WlOutput,
    name: Option<String>,
}

#[derive(Debug, Default)]
struct ExtState {
    outputs: BTreeMap<u32, OutputRecord>,
    capture_manager: Option<ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1>,
    source_manager: Option<ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1>,
    constraint_size: Option<PixelSize>,
    shm_formats: Vec<wl_shm::Format>,
    constraints_done: bool,
    session_stopped: bool,
    sync_done: bool,
}

#[derive(Debug)]
struct OutputData {
    global_name: u32,
}

pub(crate) fn negotiate_connection(
    connection: Connection,
    request: &CaptureRequest,
) -> Result<Vec<ExtCaptureConstraints>, CaptureError> {
    let display = connection.display();
    let mut event_queue = connection.new_event_queue::<ExtState>();
    let queue_handle = event_queue.handle();
    let _registry = display.get_registry(&queue_handle, ());
    let mut state = ExtState::default();

    bounded_roundtrip(
        &display,
        &queue_handle,
        &mut event_queue,
        &mut state,
        request,
        "ext-image-copy-capture registry initialization",
    )?;
    bounded_roundtrip(
        &display,
        &queue_handle,
        &mut event_queue,
        &mut state,
        request,
        "ext-image-copy-capture output initialization",
    )?;

    let capture_manager = state.capture_manager.clone().ok_or_else(|| {
        CaptureError::Unsupported("ext-image-copy-capture manager is not advertised by this compositor".into())
    })?;
    let source_manager = state.source_manager.clone().ok_or_else(|| {
        CaptureError::Unsupported("ext output image capture source manager is not advertised by this compositor".into())
    })?;
    let output_ids = select_output_ids(&state, request.screen_id.as_deref())?;
    let mut result = Vec::with_capacity(output_ids.len());

    for output_id in output_ids {
        ensure_not_cancelled(request)?;
        state.constraint_size = None;
        state.shm_formats.clear();
        state.constraints_done = false;
        state.session_stopped = false;

        let output = state
            .outputs
            .get(&output_id)
            .ok_or_else(|| CaptureError::BackendFailure("selected Wayland output disappeared".into()))?;
        let output_proxy = output.proxy.clone();
        let output_name = output.name.clone().unwrap_or_else(|| output_id.to_string());

        let source = source_manager.create_source(&output_proxy, &queue_handle, ());
        let session = capture_manager.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &queue_handle,
            (),
        );

        let deadline = Instant::now() + NEGOTIATION_TIMEOUT;
        let negotiation = wait_for_constraints(&mut event_queue, &mut state, request, deadline).and_then(|()| {
            let pixel_size = state.constraint_size.ok_or_else(|| {
                CaptureError::InvalidFrame("ext-image-copy-capture did not advertise a buffer size".into())
            })?;
            if state.shm_formats.is_empty() {
                return Err(CaptureError::Unsupported("ext-image-copy-capture advertised no SHM formats".into()));
            }
            Ok(ExtCaptureConstraints { output_id: output_name, pixel_size, shm_formats: state.shm_formats.clone() })
        });
        session.destroy();
        source.destroy();
        result.push(negotiation?);
    }

    Ok(result)
}

fn select_output_ids(state: &ExtState, requested: Option<&str>) -> Result<Vec<u32>, CaptureError> {
    if state.outputs.is_empty() {
        return Err(CaptureError::Unsupported("Wayland compositor exposes no wl_output globals".into()));
    }
    if let Some(requested) = requested {
        let id = state
            .outputs
            .iter()
            .find_map(|(id, output)| {
                (output.name.as_deref() == Some(requested) || id.to_string() == requested).then_some(*id)
            })
            .ok_or_else(|| CaptureError::Unsupported(format!("Wayland output '{requested}' was not found")))?;
        Ok(vec![id])
    } else {
        Ok(state.outputs.keys().copied().collect())
    }
}

fn ensure_not_cancelled(request: &CaptureRequest) -> Result<(), CaptureError> {
    if request.is_cancelled() { Err(CaptureError::Cancelled) } else { Ok(()) }
}

fn bounded_roundtrip(
    display: &wl_display::WlDisplay,
    queue_handle: &QueueHandle<ExtState>,
    event_queue: &mut wayland_client::EventQueue<ExtState>,
    state: &mut ExtState,
    request: &CaptureRequest,
    context: &str,
) -> Result<(), CaptureError> {
    state.sync_done = false;
    let _callback = display.sync(queue_handle, ());
    let deadline = Instant::now() + NEGOTIATION_TIMEOUT;
    while !state.sync_done {
        dispatch_with_deadline(event_queue, state, request, deadline, context)?;
    }
    Ok(())
}

fn wait_for_constraints(
    event_queue: &mut wayland_client::EventQueue<ExtState>,
    state: &mut ExtState,
    request: &CaptureRequest,
    deadline: Instant,
) -> Result<(), CaptureError> {
    while !state.constraints_done {
        if state.session_stopped {
            return Err(CaptureError::BackendFailure(
                "ext-image-copy-capture session stopped during negotiation".into(),
            ));
        }
        dispatch_with_deadline(event_queue, state, request, deadline, "ext-image-copy-capture constraint negotiation")?;
    }
    Ok(())
}

fn dispatch_with_deadline(
    event_queue: &mut wayland_client::EventQueue<ExtState>,
    state: &mut ExtState,
    request: &CaptureRequest,
    deadline: Instant,
    context: &str,
) -> Result<(), CaptureError> {
    loop {
        ensure_not_cancelled(request)?;
        let dispatched = event_queue
            .dispatch_pending(state)
            .map_err(|error| CaptureError::BackendFailure(format!("{context} dispatch failed: {error}")))?;
        if dispatched > 0 {
            return Ok(());
        }
        event_queue
            .flush()
            .map_err(|error| CaptureError::BackendFailure(format!("{context} flush failed: {error}")))?;
        let Some(read_guard) = event_queue.prepare_read() else {
            continue;
        };
        let remaining = deadline.checked_duration_since(Instant::now()).ok_or(CaptureError::Timeout)?;
        let poll_duration = remaining.min(CANCEL_POLL_SLICE);
        let timeout = Timespec {
            tv_sec: poll_duration.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(poll_duration.subsec_nanos()),
        };
        let connection_fd = read_guard.connection_fd();
        let mut poll_fds = [PollFd::from_borrowed_fd(connection_fd, PollFlags::IN | PollFlags::ERR | PollFlags::HUP)];
        match poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) => {
                drop(read_guard);
                ensure_not_cancelled(request)?;
                if Instant::now() >= deadline {
                    return Err(CaptureError::Timeout);
                }
                continue;
            }
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => {
                drop(read_guard);
                continue;
            }
            Err(error) => {
                drop(read_guard);
                return Err(CaptureError::BackendFailure(format!("{context} poll failed: {error}")));
            }
        }
        read_guard.read().map_err(|error| CaptureError::BackendFailure(format!("{context} read failed: {error}")))?;
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for ExtState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == EXT_IMAGE_COPY_CAPTURE_MANAGER => {
                state.capture_manager = Some(registry.bind(
                    name,
                    version.min(EXT_IMAGE_COPY_CAPTURE_MANAGER_MAX_VERSION),
                    queue_handle,
                    (),
                ));
            }
            wl_registry::Event::Global { name, interface, version }
                if interface == EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER =>
            {
                state.source_manager = Some(registry.bind(
                    name,
                    version.min(EXT_OUTPUT_IMAGE_CAPTURE_SOURCE_MANAGER_MAX_VERSION),
                    queue_handle,
                    (),
                ));
            }
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                let proxy = registry.bind(
                    name,
                    version.min(WL_OUTPUT_MAX_VERSION),
                    queue_handle,
                    OutputData { global_name: name },
                );
                state.outputs.insert(name, OutputRecord { proxy, name: None });
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.outputs.remove(&name);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, OutputData> for ExtState {
    fn event(
        state: &mut Self,
        _output: &wl_output::WlOutput,
        event: wl_output::Event,
        data: &OutputData,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(output) = state.outputs.get_mut(&data.global_name)
        {
            output.name = Some(name);
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for ExtState {
    fn event(
        state: &mut Self,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.sync_done = true;
        }
    }
}

impl Dispatch<ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1, ()> for ExtState {
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

impl Dispatch<ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1, ()> for ExtState {
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

impl Dispatch<ext_image_capture_source_v1::ExtImageCaptureSourceV1, ()> for ExtState {
    fn event(
        _state: &mut Self,
        _source: &ext_image_capture_source_v1::ExtImageCaptureSourceV1,
        _event: ext_image_capture_source_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, ()> for ExtState {
    fn event(
        state: &mut Self,
        _session: &ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } if width > 0 && height > 0 => {
                state.constraint_size = Some(PixelSize { width, height });
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat { format: WEnum::Value(format) } => {
                state.shm_formats.push(format);
            }
            ext_image_copy_capture_session_v1::Event::Done => {
                state.constraints_done = true;
            }
            ext_image_copy_capture_session_v1::Event::Stopped => {
                state.session_stopped = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lisnap_core::CaptureCancellation;

    #[test]
    fn cancellation_is_observed_before_waiting() {
        let cancellation = CaptureCancellation::default();
        let request = CaptureRequest { cancellation: Some(cancellation.clone()), ..CaptureRequest::default() };
        assert!(ensure_not_cancelled(&request).is_ok());
        cancellation.cancel();
        assert!(matches!(ensure_not_cancelled(&request), Err(CaptureError::Cancelled)));
    }
}
