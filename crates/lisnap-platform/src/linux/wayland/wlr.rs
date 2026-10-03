use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::fd::AsFd,
    time::{Duration, Instant, SystemTime},
};

use lisnap_core::{CaptureError, CaptureFrame, CaptureRequest, PixelSize, RgbaImage, ScreenInfo};
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::{MemfdFlags, memfd_create},
    time::Timespec,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{wl_buffer, wl_callback, wl_display, wl_output, wl_registry, wl_shm, wl_shm_pool},
};
use wayland_protocols_wlr::screencopy::v1::client::{zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1};

const WL_OUTPUT_MAX_VERSION: u32 = 4;
const WLR_SCREENCOPY_MANAGER_MAX_VERSION: u32 = 3;
const WLR_SCREENCOPY_MANAGER: &str = "zwlr_screencopy_manager_v1";
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_POLL_SLICE: Duration = Duration::from_millis(50);

#[derive(Debug)]
struct OutputRecord {
    proxy: wl_output::WlOutput,
    name: Option<String>,
    description: Option<String>,
    make: Option<String>,
    model: Option<String>,
    scale_factor: Option<i32>,
}

#[derive(Debug, Clone, Copy)]
struct BufferSpec {
    format: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameState {
    Waiting,
    Ready,
    Failed,
}

#[derive(Debug, Default)]
struct CaptureState {
    outputs: BTreeMap<u32, OutputRecord>,
    manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    manager_version: Option<u32>,
    shm: Option<wl_shm::WlShm>,
    buffer_spec: Option<BufferSpec>,
    buffer_done: bool,
    y_invert: bool,
    frame_state: Option<FrameState>,
    sync_done: bool,
}

#[derive(Debug)]
struct OutputData {
    global_name: u32,
}

pub(crate) fn capture_connection(
    connection: Connection,
    request: &CaptureRequest,
) -> Result<Vec<CaptureFrame>, CaptureError> {
    let display = connection.display();
    let mut event_queue = connection.new_event_queue::<CaptureState>();
    let queue_handle = event_queue.handle();
    let _registry = display.get_registry(&queue_handle, ());
    let mut state = CaptureState::default();

    bounded_roundtrip(
        &display,
        &queue_handle,
        &mut event_queue,
        &mut state,
        request,
        "Wayland registry initialization",
    )?;
    bounded_roundtrip(&display, &queue_handle, &mut event_queue, &mut state, request, "Wayland output initialization")?;

    let manager = state
        .manager
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("wlr-screencopy is not advertised by this compositor".into()))?;
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("wl_shm is not advertised by this compositor".into()))?;

    let output_ids = select_output_ids(&state, request.screen_id.as_deref())?;
    let mut frames = Vec::with_capacity(output_ids.len());
    for output_id in output_ids {
        ensure_not_cancelled(request)?;
        state.buffer_spec = None;
        state.buffer_done = false;
        state.y_invert = false;
        state.frame_state = Some(FrameState::Waiting);

        let output = state
            .outputs
            .get(&output_id)
            .ok_or_else(|| CaptureError::BackendFailure("selected Wayland output disappeared".into()))?;
        let output_proxy = output.proxy.clone();
        let frame = manager.capture_output(i32::from(request.include_cursor), &output_proxy, &queue_handle, ());
        let deadline = Instant::now() + CAPTURE_TIMEOUT;
        let capture_result = (|| -> Result<CaptureFrame, CaptureError> {
            wait_for_buffer_offer(&mut event_queue, &mut state, request, deadline)?;
            let spec = state
                .buffer_spec
                .ok_or_else(|| CaptureError::InvalidFrame("wlr-screencopy did not offer a SHM buffer".into()))?;
            validate_spec(spec)?;

            let byte_len = (spec.stride as usize)
                .checked_mul(spec.height as usize)
                .ok_or_else(|| CaptureError::InvalidFrame("wlr-screencopy SHM buffer size overflow".into()))?;
            let size_i32 = i32::try_from(byte_len)
                .map_err(|_| CaptureError::InvalidFrame("wlr-screencopy SHM buffer exceeds protocol size".into()))?;
            let width_i32 = i32::try_from(spec.width)
                .map_err(|_| CaptureError::InvalidFrame("wlr-screencopy width exceeds protocol range".into()))?;
            let height_i32 = i32::try_from(spec.height)
                .map_err(|_| CaptureError::InvalidFrame("wlr-screencopy height exceeds protocol range".into()))?;
            let stride_i32 = i32::try_from(spec.stride)
                .map_err(|_| CaptureError::InvalidFrame("wlr-screencopy stride exceeds protocol range".into()))?;

            let name = CString::new("lisnap-screencopy")
                .map_err(|_| CaptureError::BackendFailure("invalid memfd name".into()))?;
            let fd = memfd_create(name.as_c_str(), MemfdFlags::CLOEXEC)
                .map_err(|error| CaptureError::BackendFailure(format!("memfd_create failed: {error}")))?;
            let mut file = File::from(fd);
            file.set_len(byte_len as u64)
                .map_err(|error| CaptureError::BackendFailure(format!("resizing SHM buffer failed: {error}")))?;

            let pool = shm.create_pool(file.as_fd(), size_i32, &queue_handle, ());
            let buffer = pool.create_buffer(0, width_i32, height_i32, stride_i32, spec.format, &queue_handle, ());
            pool.destroy();

            let buffer_result = (|| -> Result<CaptureFrame, CaptureError> {
                frame.copy(&buffer);
                wait_for_frame(&mut event_queue, &mut state, request, deadline)?;

                let mut native = vec![0_u8; byte_len];
                file.seek(SeekFrom::Start(0))
                    .map_err(|error| CaptureError::BackendFailure(format!("seeking SHM buffer failed: {error}")))?;
                file.read_exact(&mut native)
                    .map_err(|error| CaptureError::BackendFailure(format!("reading SHM buffer failed: {error}")))?;

                let pixels = convert_shm_rgba(spec, &native, state.y_invert)?;
                let image = RgbaImage::new(PixelSize { width: spec.width, height: spec.height }, pixels)?;
                let output = state
                    .outputs
                    .get(&output_id)
                    .ok_or_else(|| CaptureError::BackendFailure("captured Wayland output disappeared".into()))?;
                let stable_id = output.name.clone().unwrap_or_else(|| output_id.to_string());
                let display_name = output
                    .description
                    .clone()
                    .or_else(|| output.name.clone())
                    .or_else(|| match (&output.make, &output.model) {
                        (Some(make), Some(model)) => Some(format!("{make} {model}")),
                        _ => None,
                    })
                    .unwrap_or_else(|| stable_id.clone());
                Ok(CaptureFrame {
                    screen: ScreenInfo {
                        id: stable_id,
                        name: display_name,
                        logical_geometry: None,
                        pixel_size: PixelSize { width: spec.width, height: spec.height },
                        scale_factor: output.scale_factor.map(f64::from),
                    },
                    image,
                    captured_at: SystemTime::now(),
                })
            })();
            buffer.destroy();
            buffer_result
        })();
        frame.destroy();
        frames.push(capture_result?);
    }

    Ok(frames)
}

fn select_output_ids(state: &CaptureState, requested: Option<&str>) -> Result<Vec<u32>, CaptureError> {
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
    queue_handle: &QueueHandle<CaptureState>,
    event_queue: &mut wayland_client::EventQueue<CaptureState>,
    state: &mut CaptureState,
    request: &CaptureRequest,
    context: &str,
) -> Result<(), CaptureError> {
    state.sync_done = false;
    let _callback = display.sync(queue_handle, ());
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    while !state.sync_done {
        dispatch_with_deadline(event_queue, state, request, deadline, context)?;
    }
    Ok(())
}

fn wait_for_buffer_offer(
    event_queue: &mut wayland_client::EventQueue<CaptureState>,
    state: &mut CaptureState,
    request: &CaptureRequest,
    deadline: Instant,
) -> Result<(), CaptureError> {
    loop {
        if state.frame_state == Some(FrameState::Failed) {
            return Err(CaptureError::BackendFailure("wlr-screencopy frame negotiation failed".into()));
        }
        let manager_version = state.manager_version.unwrap_or(1);
        if state.buffer_spec.is_some() && (manager_version < 3 || state.buffer_done) {
            return Ok(());
        }
        dispatch_with_deadline(event_queue, state, request, deadline, "wlr-screencopy negotiation")?;
    }
}

fn wait_for_frame(
    event_queue: &mut wayland_client::EventQueue<CaptureState>,
    state: &mut CaptureState,
    request: &CaptureRequest,
    deadline: Instant,
) -> Result<(), CaptureError> {
    loop {
        match state.frame_state {
            Some(FrameState::Ready) => return Ok(()),
            Some(FrameState::Failed) => {
                return Err(CaptureError::BackendFailure("wlr-screencopy compositor rejected the copy".into()));
            }
            _ => dispatch_with_deadline(event_queue, state, request, deadline, "wlr-screencopy copy")?,
        }
    }
}

fn dispatch_with_deadline(
    event_queue: &mut wayland_client::EventQueue<CaptureState>,
    state: &mut CaptureState,
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

fn validate_spec(spec: BufferSpec) -> Result<(), CaptureError> {
    let min_stride = spec
        .width
        .checked_mul(4)
        .ok_or_else(|| CaptureError::InvalidFrame("wlr-screencopy row size overflow".into()))?;
    if spec.width == 0 || spec.height == 0 || spec.stride < min_stride {
        return Err(CaptureError::InvalidFrame("invalid wlr-screencopy SHM dimensions or stride".into()));
    }
    match spec.format {
        wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => Ok(()),
        _ => {
            Err(CaptureError::Unsupported(format!("wlr-screencopy SHM format {:?} is not supported yet", spec.format)))
        }
    }
}

fn convert_shm_rgba(spec: BufferSpec, native: &[u8], y_invert: bool) -> Result<Vec<u8>, CaptureError> {
    super::shm::convert_rgba(
        spec.format,
        PixelSize { width: spec.width, height: spec.height },
        spec.stride,
        native,
        y_invert,
    )
}

impl Dispatch<wl_callback::WlCallback, ()> for CaptureState {
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

impl Dispatch<wl_registry::WlRegistry, ()> for CaptureState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == WLR_SCREENCOPY_MANAGER => {
                state.manager_version = Some(version);
                state.manager = Some(registry.bind::<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, _>(
                    name,
                    version.min(WLR_SCREENCOPY_MANAGER_MAX_VERSION),
                    queue_handle,
                    (),
                ));
            }
            wl_registry::Event::Global { name, interface, version } if interface == "wl_shm" => {
                state.shm = Some(registry.bind::<wl_shm::WlShm, _, _>(name, version.min(1), queue_handle, ()));
            }
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                let proxy = registry.bind::<wl_output::WlOutput, _, _>(
                    name,
                    version.min(WL_OUTPUT_MAX_VERSION),
                    queue_handle,
                    OutputData { global_name: name },
                );
                state.outputs.insert(
                    name,
                    OutputRecord { proxy, name: None, description: None, make: None, model: None, scale_factor: None },
                );
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.outputs.remove(&name);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, OutputData> for CaptureState {
    fn event(
        state: &mut Self,
        _output: &wl_output::WlOutput,
        event: wl_output::Event,
        data: &OutputData,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(&data.global_name) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { make, model, .. } => {
                output.make = Some(make);
                output.model = Some(model);
            }
            wl_output::Event::Scale { factor } => output.scale_factor = Some(factor),
            wl_output::Event::Name { name } => output.name = Some(name),
            wl_output::Event::Description { description } => output.description = Some(description),
            _ => {}
        }
    }
}

impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _event: zwlr_screencopy_manager_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer { format, width, height, stride } => {
                if let WEnum::Value(format) = format {
                    state.buffer_spec = Some(BufferSpec { format, width, height, stride });
                } else {
                    state.frame_state = Some(FrameState::Failed);
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => state.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Flags { flags: WEnum::Value(flags) } => {
                state.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.frame_state = Some(FrameState::Ready),
            zwlr_screencopy_frame_v1::Event::Failed => state.frame_state = Some(FrameState::Failed),
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm_pool::WlShmPool,
        _event: wl_shm_pool::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_buffer::WlBuffer,
        _event: wl_buffer::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lisnap_core::CaptureCancellation;

    #[test]
    fn reports_cancelled_request() {
        let cancellation = CaptureCancellation::default();
        let request = CaptureRequest { cancellation: Some(cancellation.clone()), ..CaptureRequest::default() };
        assert!(ensure_not_cancelled(&request).is_ok());
        cancellation.cancel();
        assert!(matches!(ensure_not_cancelled(&request), Err(CaptureError::Cancelled)));
    }

    #[test]
    fn converts_xrgb_stride_and_bgr_order() {
        let spec = BufferSpec { format: wl_shm::Format::Xrgb8888, width: 2, height: 1, stride: 12 };
        let native = [3, 2, 1, 0, 30, 20, 10, 0, 9, 9, 9, 9];
        assert_eq!(convert_shm_rgba(spec, &native, false).unwrap(), [1, 2, 3, 255, 10, 20, 30, 255]);
    }

    #[test]
    fn flips_y_inverted_frames() {
        let spec = BufferSpec { format: wl_shm::Format::Xrgb8888, width: 1, height: 2, stride: 4 };
        let native = [3, 2, 1, 0, 30, 20, 10, 0];
        assert_eq!(convert_shm_rgba(spec, &native, true).unwrap(), [10, 20, 30, 255, 1, 2, 3, 255]);
    }

    #[test]
    fn unpremultiplies_argb() {
        let spec = BufferSpec { format: wl_shm::Format::Argb8888, width: 1, height: 1, stride: 4 };
        let native = [25, 50, 100, 128];
        let rgba = convert_shm_rgba(spec, &native, false).unwrap();
        assert_eq!(rgba[3], 128);
        assert!((198..=200).contains(&rgba[0]));
        assert!((99..=101).contains(&rgba[1]));
        assert!((49..=51).contains(&rgba[2]));
    }
}
