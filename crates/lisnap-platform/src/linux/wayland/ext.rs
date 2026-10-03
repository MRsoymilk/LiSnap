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
use wayland_protocols::ext::{
    image_capture_source::v1::client::{ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1},
    image_copy_capture::v1::client::{
        ext_image_copy_capture_frame_v1, ext_image_copy_capture_manager_v1, ext_image_copy_capture_session_v1,
    },
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
    constraint_generation: u64,
    session_stopped: bool,
    shm: Option<wl_shm::WlShm>,
    frame_state: Option<FrameState>,
    frame_transform: Option<FrameTransform>,
    sync_done: bool,
}

#[derive(Debug)]
struct OutputData {
    global_name: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameFailure {
    Unknown,
    BufferConstraints,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameState {
    Waiting,
    Ready,
    Failed(FrameFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameTransform {
    Known(wl_output::Transform),
    Unknown,
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

pub(crate) fn capture_connection(
    connection: Connection,
    request: &CaptureRequest,
) -> Result<Vec<CaptureFrame>, CaptureError> {
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
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("wl_shm is not advertised by this compositor".into()))?;
    let output_ids = select_output_ids(&state, request.screen_id.as_deref())?;
    let mut frames = Vec::with_capacity(output_ids.len());

    for output_id in output_ids {
        ensure_not_cancelled(request)?;
        state.constraint_size = None;
        state.shm_formats.clear();
        state.constraints_done = false;
        state.session_stopped = false;
        state.frame_state = None;
        state.frame_transform = None;

        let output = state
            .outputs
            .get(&output_id)
            .ok_or_else(|| CaptureError::BackendFailure("selected Wayland output disappeared".into()))?;
        let output_proxy = output.proxy.clone();
        let output_name = output.name.clone().unwrap_or_else(|| output_id.to_string());

        let source = source_manager.create_source(&output_proxy, &queue_handle, ());
        let options = if request.include_cursor {
            ext_image_copy_capture_manager_v1::Options::PaintCursors
        } else {
            ext_image_copy_capture_manager_v1::Options::empty()
        };
        let session = capture_manager.create_session(&source, options, &queue_handle, ());
        let capture_result = (|| -> Result<CaptureFrame, CaptureError> {
            for attempt in 0..2 {
                wait_for_constraints(&mut event_queue, &mut state, request, Instant::now() + NEGOTIATION_TIMEOUT)?;
                let generation = state.constraint_generation;
                match capture_frame_attempt(
                    &session,
                    &shm,
                    &queue_handle,
                    &mut event_queue,
                    &mut state,
                    request,
                    &output_name,
                )? {
                    Some(frame) => return Ok(frame),
                    None if attempt == 0 => {
                        if state.constraint_generation == generation {
                            state.constraints_done = false;
                        }
                    }
                    None => {
                        return Err(CaptureError::InvalidFrame(
                            "ext-image-copy-capture buffer constraints changed repeatedly".into(),
                        ));
                    }
                }
            }
            unreachable!("two ext-image-copy-capture attempts always return or error")
        })();
        session.destroy();
        source.destroy();
        frames.push(capture_result?);
    }

    Ok(frames)
}

fn capture_frame_attempt(
    session: &ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
    shm: &wl_shm::WlShm,
    queue_handle: &QueueHandle<ExtState>,
    event_queue: &mut wayland_client::EventQueue<ExtState>,
    state: &mut ExtState,
    request: &CaptureRequest,
    output_name: &str,
) -> Result<Option<CaptureFrame>, CaptureError> {
    let size = state
        .constraint_size
        .ok_or_else(|| CaptureError::InvalidFrame("ext-image-copy-capture did not advertise a buffer size".into()))?;
    let format = select_shm_format(&state.shm_formats)?;
    let stride = size
        .width
        .checked_mul(4)
        .ok_or_else(|| CaptureError::InvalidFrame("ext-image-copy-capture row size overflow".into()))?;
    let byte_len = (stride as usize)
        .checked_mul(size.height as usize)
        .ok_or_else(|| CaptureError::InvalidFrame("ext-image-copy-capture buffer size overflow".into()))?;
    let size_i32 = i32::try_from(byte_len)
        .map_err(|_| CaptureError::InvalidFrame("ext-image-copy-capture buffer exceeds protocol size".into()))?;
    let width_i32 = i32::try_from(size.width)
        .map_err(|_| CaptureError::InvalidFrame("ext-image-copy-capture width exceeds protocol range".into()))?;
    let height_i32 = i32::try_from(size.height)
        .map_err(|_| CaptureError::InvalidFrame("ext-image-copy-capture height exceeds protocol range".into()))?;
    let stride_i32 = i32::try_from(stride)
        .map_err(|_| CaptureError::InvalidFrame("ext-image-copy-capture stride exceeds protocol range".into()))?;

    let name =
        CString::new("lisnap-ext-capture").map_err(|_| CaptureError::BackendFailure("invalid memfd name".into()))?;
    let fd = memfd_create(name.as_c_str(), MemfdFlags::CLOEXEC)
        .map_err(|error| CaptureError::BackendFailure(format!("memfd_create failed: {error}")))?;
    let mut file = File::from(fd);
    file.set_len(byte_len as u64)
        .map_err(|error| CaptureError::BackendFailure(format!("resizing ext SHM buffer failed: {error}")))?;

    let pool = shm.create_pool(file.as_fd(), size_i32, queue_handle, ());
    let buffer = pool.create_buffer(0, width_i32, height_i32, stride_i32, format, queue_handle, ());
    pool.destroy();

    state.frame_state = Some(FrameState::Waiting);
    state.frame_transform = None;
    let frame = session.create_frame(queue_handle, ());
    let frame_result = (|| -> Result<Option<CaptureFrame>, CaptureError> {
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, width_i32, height_i32);
        frame.capture();
        match wait_for_frame(event_queue, state, request, Instant::now() + NEGOTIATION_TIMEOUT)? {
            FrameState::Failed(FrameFailure::BufferConstraints) => return Ok(None),
            FrameState::Failed(FrameFailure::Stopped) => {
                return Err(CaptureError::BackendFailure(
                    "ext-image-copy-capture session stopped while copying".into(),
                ));
            }
            FrameState::Failed(FrameFailure::Unknown) => {
                return Err(CaptureError::BackendFailure(
                    "ext-image-copy-capture compositor rejected the frame".into(),
                ));
            }
            FrameState::Ready => {}
            FrameState::Waiting => {
                return Err(CaptureError::BackendFailure(
                    "ext-image-copy-capture frame remained pending after wait".into(),
                ));
            }
        }

        let mut native = vec![0_u8; byte_len];
        file.seek(SeekFrom::Start(0))
            .map_err(|error| CaptureError::BackendFailure(format!("seeking ext SHM buffer failed: {error}")))?;
        file.read_exact(&mut native)
            .map_err(|error| CaptureError::BackendFailure(format!("reading ext SHM buffer failed: {error}")))?;

        let rgba = convert_shm_rgba(format, size, stride, &native)?;
        let transform = match state.frame_transform {
            Some(FrameTransform::Known(transform)) => transform,
            Some(FrameTransform::Unknown) => {
                return Err(CaptureError::Unsupported(
                    "ext-image-copy-capture returned an unknown Wayland transform".into(),
                ));
            }
            None => {
                return Err(CaptureError::InvalidFrame(
                    "ext-image-copy-capture ready frame omitted transform metadata".into(),
                ));
            }
        };
        let (pixel_size, rgba) = normalize_transform(size, rgba, transform)?;
        let image = RgbaImage::new(pixel_size, rgba)?;
        Ok(Some(CaptureFrame {
            screen: ScreenInfo {
                id: output_name.to_owned(),
                name: output_name.to_owned(),
                logical_geometry: None,
                pixel_size,
                scale_factor: None,
            },
            image,
            captured_at: SystemTime::now(),
        }))
    })();
    frame.destroy();
    buffer.destroy();
    frame_result
}

fn select_shm_format(formats: &[wl_shm::Format]) -> Result<wl_shm::Format, CaptureError> {
    if formats.contains(&wl_shm::Format::Argb8888) {
        Ok(wl_shm::Format::Argb8888)
    } else if formats.contains(&wl_shm::Format::Xrgb8888) {
        Ok(wl_shm::Format::Xrgb8888)
    } else {
        Err(CaptureError::Unsupported(format!("ext-image-copy-capture has no supported SHM format: {formats:?}")))
    }
}

fn wait_for_frame(
    event_queue: &mut wayland_client::EventQueue<ExtState>,
    state: &mut ExtState,
    request: &CaptureRequest,
    deadline: Instant,
) -> Result<FrameState, CaptureError> {
    loop {
        if state.session_stopped {
            return Ok(FrameState::Failed(FrameFailure::Stopped));
        }
        match state.frame_state {
            Some(FrameState::Ready) => return Ok(FrameState::Ready),
            Some(FrameState::Failed(reason)) => return Ok(FrameState::Failed(reason)),
            _ => dispatch_with_deadline(event_queue, state, request, deadline, "ext-image-copy-capture frame copy")?,
        }
    }
}

fn convert_shm_rgba(
    format: wl_shm::Format,
    size: PixelSize,
    stride: u32,
    native: &[u8],
) -> Result<Vec<u8>, CaptureError> {
    let needed = (stride as usize)
        .checked_mul(size.height as usize)
        .ok_or_else(|| CaptureError::InvalidFrame("ext-image-copy-capture byte length overflow".into()))?;
    if native.len() < needed {
        return Err(CaptureError::InvalidFrame("ext-image-copy-capture SHM data is shorter than its stride".into()));
    }
    let row_bytes = (size.width as usize)
        .checked_mul(4)
        .ok_or_else(|| CaptureError::InvalidFrame("ext-image-copy-capture row size overflow".into()))?;
    if (stride as usize) < row_bytes {
        return Err(CaptureError::InvalidFrame("ext-image-copy-capture stride is shorter than one row".into()));
    }
    let mut rgba = vec![0_u8; row_bytes * size.height as usize];
    for y in 0..size.height as usize {
        let src = &native[y * stride as usize..y * stride as usize + row_bytes];
        let dst = &mut rgba[y * row_bytes..(y + 1) * row_bytes];
        for (source, target) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            let alpha = if format == wl_shm::Format::Argb8888 { source[3] } else { 255 };
            let (red, green, blue) = if alpha == 0 {
                (0, 0, 0)
            } else if alpha < 255 && format == wl_shm::Format::Argb8888 {
                (unpremultiply(source[2], alpha), unpremultiply(source[1], alpha), unpremultiply(source[0], alpha))
            } else {
                (source[2], source[1], source[0])
            };
            target.copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Ok(rgba)
}

fn unpremultiply(channel: u8, alpha: u8) -> u8 {
    ((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255) as u8
}

fn normalize_transform(
    size: PixelSize,
    rgba: Vec<u8>,
    transform: wl_output::Transform,
) -> Result<(PixelSize, Vec<u8>), CaptureError> {
    let expected = (size.width as usize)
        .checked_mul(size.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| CaptureError::InvalidFrame("ext-image-copy-capture RGBA size overflow".into()))?;
    if rgba.len() != expected {
        return Err(CaptureError::InvalidFrame("ext-image-copy-capture RGBA length does not match dimensions".into()));
    }
    if transform == wl_output::Transform::Normal {
        return Ok((size, rgba));
    }
    let rotated = matches!(
        transform,
        wl_output::Transform::_90
            | wl_output::Transform::_270
            | wl_output::Transform::Flipped90
            | wl_output::Transform::Flipped270
    );
    let dst_size = if rotated { PixelSize { width: size.height, height: size.width } } else { size };
    let mut dst = vec![0_u8; rgba.len()];
    for y in 0..size.height {
        for x in 0..size.width {
            let (dx, dy) = match transform {
                wl_output::Transform::Normal => (x, y),
                wl_output::Transform::_90 => (y, size.width - 1 - x),
                wl_output::Transform::_180 => (size.width - 1 - x, size.height - 1 - y),
                wl_output::Transform::_270 => (size.height - 1 - y, x),
                wl_output::Transform::Flipped => (size.width - 1 - x, y),
                wl_output::Transform::Flipped90 => (y, x),
                wl_output::Transform::Flipped180 => (x, size.height - 1 - y),
                wl_output::Transform::Flipped270 => (size.height - 1 - y, size.width - 1 - x),
                _ => {
                    return Err(CaptureError::Unsupported(format!("unknown Wayland output transform {transform:?}")));
                }
            };
            let src = ((y * size.width + x) * 4) as usize;
            let dst_index = ((dy * dst_size.width + dx) * 4) as usize;
            dst[dst_index..dst_index + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    Ok((dst_size, dst))
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

fn begin_constraint_batch(state: &mut ExtState) {
    if state.constraints_done {
        state.constraints_done = false;
        state.constraint_size = None;
        state.shm_formats.clear();
    }
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
            wl_registry::Event::Global { name, interface, version } if interface == "wl_shm" => {
                state.shm = Some(registry.bind(name, version.min(1), queue_handle, ()));
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

impl Dispatch<wl_shm::WlShm, ()> for ExtState {
    fn event(
        _state: &mut Self,
        _shm: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for ExtState {
    fn event(
        _state: &mut Self,
        _pool: &wl_shm_pool::WlShmPool,
        _event: wl_shm_pool::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for ExtState {
    fn event(
        _state: &mut Self,
        _buffer: &wl_buffer::WlBuffer,
        _event: wl_buffer::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1, ()> for ExtState {
    fn event(
        state: &mut Self,
        _frame: &ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_frame_v1::Event::Transform { transform } => {
                state.frame_transform = Some(match transform {
                    WEnum::Value(transform) => FrameTransform::Known(transform),
                    _ => FrameTransform::Unknown,
                });
            }
            ext_image_copy_capture_frame_v1::Event::Ready => {
                state.frame_state = Some(FrameState::Ready);
            }
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                let failure = match reason {
                    WEnum::Value(ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints) => {
                        FrameFailure::BufferConstraints
                    }
                    WEnum::Value(ext_image_copy_capture_frame_v1::FailureReason::Stopped) => FrameFailure::Stopped,
                    _ => FrameFailure::Unknown,
                };
                state.frame_state = Some(FrameState::Failed(failure));
            }
            _ => {}
        }
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
                begin_constraint_batch(state);
                state.constraint_size = Some(PixelSize { width, height });
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat { format: WEnum::Value(format) } => {
                begin_constraint_batch(state);
                state.shm_formats.push(format);
            }
            ext_image_copy_capture_session_v1::Event::Done => {
                state.constraints_done = true;
                state.constraint_generation = state.constraint_generation.saturating_add(1);
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

    #[test]
    fn converts_xrgb_to_rgba() {
        let size = PixelSize { width: 2, height: 1 };
        let native = [3, 2, 1, 0, 30, 20, 10, 0];
        let rgba = convert_shm_rgba(wl_shm::Format::Xrgb8888, size, 8, &native).unwrap();
        assert_eq!(rgba, vec![1, 2, 3, 255, 10, 20, 30, 255]);
    }

    fn labeled_rgba(values: &[u8]) -> Vec<u8> {
        values.iter().flat_map(|value| [*value, 0, 0, 255]).collect()
    }

    fn red_channel(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks_exact(4).map(|pixel| pixel[0]).collect()
    }

    #[test]
    fn normalizes_all_wayland_transforms() {
        let size = PixelSize { width: 2, height: 3 };
        let source = labeled_rgba(&[1, 2, 3, 4, 5, 6]);
        let cases = [
            (wl_output::Transform::Normal, PixelSize { width: 2, height: 3 }, vec![1, 2, 3, 4, 5, 6]),
            (wl_output::Transform::_90, PixelSize { width: 3, height: 2 }, vec![2, 4, 6, 1, 3, 5]),
            (wl_output::Transform::_180, PixelSize { width: 2, height: 3 }, vec![6, 5, 4, 3, 2, 1]),
            (wl_output::Transform::_270, PixelSize { width: 3, height: 2 }, vec![5, 3, 1, 6, 4, 2]),
            (wl_output::Transform::Flipped, PixelSize { width: 2, height: 3 }, vec![2, 1, 4, 3, 6, 5]),
            (wl_output::Transform::Flipped90, PixelSize { width: 3, height: 2 }, vec![1, 3, 5, 2, 4, 6]),
            (wl_output::Transform::Flipped180, PixelSize { width: 2, height: 3 }, vec![5, 6, 3, 4, 1, 2]),
            (wl_output::Transform::Flipped270, PixelSize { width: 3, height: 2 }, vec![6, 4, 2, 5, 3, 1]),
        ];
        for (transform, expected_size, expected_red) in cases {
            let (actual_size, transformed) = normalize_transform(size, source.clone(), transform).unwrap();
            assert_eq!(actual_size, expected_size, "{transform:?}");
            assert_eq!(red_channel(&transformed), expected_red, "{transform:?}");
        }
    }

    #[test]
    fn starts_fresh_constraint_batch_after_done() {
        let mut state = ExtState {
            constraint_size: Some(PixelSize { width: 1920, height: 1080 }),
            shm_formats: vec![wl_shm::Format::Argb8888],
            constraints_done: true,
            constraint_generation: 1,
            ..ExtState::default()
        };
        begin_constraint_batch(&mut state);
        assert!(!state.constraints_done);
        assert_eq!(state.constraint_size, None);
        assert!(state.shm_formats.is_empty());
        assert_eq!(state.constraint_generation, 1);
    }

    #[test]
    fn chooses_argb_before_xrgb() {
        assert_eq!(
            select_shm_format(&[wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888]).unwrap(),
            wl_shm::Format::Argb8888
        );
    }
}
