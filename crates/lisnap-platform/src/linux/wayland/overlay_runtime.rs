use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::Write,
    os::fd::AsFd,
    time::{Duration, Instant},
};

use lisnap_core::{CaptureError, CaptureFrame, LogicalRect, PixelRect, PixelSize, selection_to_pixels};
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::{MemfdFlags, memfd_create},
    time::Timespec,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_display, wl_keyboard, wl_output, wl_pointer, wl_region, wl_registry,
        wl_seat, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
    },
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const WL_COMPOSITOR_MAX_VERSION: u32 = 6;
const WL_OUTPUT_MAX_VERSION: u32 = 4;
const WL_SEAT_MAX_VERSION: u32 = 9;
const LAYER_SHELL_MAX_VERSION: u32 = 5;
const LAYER_SHELL: &str = "zwlr_layer_shell_v1";
const VIEWPORTER: &str = "wp_viewporter";
const BTN_LEFT: u32 = 0x110;
const ESC_KEY: u32 = 1;
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrozenOverlayOutcome {
    Selected(PixelRect),
    Cancelled,
    Closed,
}

#[derive(Debug)]
struct OutputRecord {
    proxy: wl_output::WlOutput,
    name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RectI {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug)]
struct OverlayQuad {
    surface: wl_surface::WlSurface,
    subsurface: wl_subsurface::WlSubsurface,
    viewport: wp_viewport::WpViewport,
}

#[derive(Debug)]
struct OverlayVisuals {
    mask_quads: Vec<OverlayQuad>,
    border_quads: Vec<OverlayQuad>,
    mask_buffer: wl_buffer::WlBuffer,
    border_buffer: wl_buffer::WlBuffer,
    _solid_file: File,
}

#[derive(Debug, Default)]
struct RuntimeState {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    shm: Option<wl_shm::WlShm>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    outputs: BTreeMap<u32, OutputRecord>,
    sync_done: bool,
    configure: Option<(u32, u32, u32)>,
    logical_size: Option<PixelSize>,
    frame_size: Option<PixelSize>,
    pointer_position: Option<(f64, f64)>,
    drag_anchor: Option<(f64, f64)>,
    outcome: Option<FrozenOverlayOutcome>,
    visuals: Option<OverlayVisuals>,
    closed: bool,
}

#[derive(Debug)]
struct OutputData {
    global_name: u32,
}

impl OverlayVisuals {
    fn new(
        compositor: &wl_compositor::WlCompositor,
        subcompositor: &wl_subcompositor::WlSubcompositor,
        viewporter: &wp_viewporter::WpViewporter,
        shm: &wl_shm::WlShm,
        parent: &wl_surface::WlSurface,
        logical_size: PixelSize,
        queue_handle: &QueueHandle<RuntimeState>,
    ) -> Result<Self, CaptureError> {
        let name = CString::new("lisnap-overlay-visuals")
            .map_err(|_| CaptureError::BackendFailure("invalid overlay visual memfd name".into()))?;
        let fd = memfd_create(name.as_c_str(), MemfdFlags::CLOEXEC)
            .map_err(|error| CaptureError::BackendFailure(format!("overlay visual memfd_create failed: {error}")))?;
        let mut file = File::from(fd);
        file.set_len(8)
            .map_err(|error| CaptureError::BackendFailure(format!("resizing overlay visual SHM failed: {error}")))?;
        file.write_all(&[0, 0, 0, 144, 255, 255, 255, 0])
            .map_err(|error| CaptureError::BackendFailure(format!("writing overlay visual SHM failed: {error}")))?;

        let pool = shm.create_pool(file.as_fd(), 8, queue_handle, ());
        let mask_buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, queue_handle, ());
        let border_buffer = pool.create_buffer(4, 1, 1, 4, wl_shm::Format::Xrgb8888, queue_handle, ());
        pool.destroy();

        let mut mask_quads = Vec::with_capacity(4);
        let mut border_quads = Vec::with_capacity(4);
        for _ in 0..4 {
            mask_quads.push(Self::create_quad(compositor, subcompositor, viewporter, parent, queue_handle));
            border_quads.push(Self::create_quad(compositor, subcompositor, viewporter, parent, queue_handle));
        }

        let visuals = Self { mask_quads, border_quads, mask_buffer, border_buffer, _solid_file: file };
        visuals.update_selection(None, None, logical_size);
        Ok(visuals)
    }

    fn create_quad(
        compositor: &wl_compositor::WlCompositor,
        subcompositor: &wl_subcompositor::WlSubcompositor,
        viewporter: &wp_viewporter::WpViewporter,
        parent: &wl_surface::WlSurface,
        queue_handle: &QueueHandle<RuntimeState>,
    ) -> OverlayQuad {
        let surface = compositor.create_surface(queue_handle, ());
        let empty_input = compositor.create_region(queue_handle, ());
        surface.set_input_region(Some(&empty_input));
        empty_input.destroy();
        let subsurface = subcompositor.get_subsurface(&surface, parent, queue_handle, ());
        subsurface.set_desync();
        let viewport = viewporter.get_viewport(&surface, queue_handle, ());
        OverlayQuad { surface, subsurface, viewport }
    }

    fn update_selection(&self, start: Option<(f64, f64)>, end: Option<(f64, f64)>, logical_size: PixelSize) {
        let width = i32::try_from(logical_size.width).unwrap_or(i32::MAX);
        let height = i32::try_from(logical_size.height).unwrap_or(i32::MAX);
        let full = RectI { x: 0, y: 0, width, height };
        let Some((start, end)) = start.zip(end) else {
            self.show_full_mask(full);
            return;
        };
        let Some((masks, borders)) = selection_visual_rects(start, end, logical_size) else {
            self.show_full_mask(full);
            return;
        };

        for (quad, rect) in self.mask_quads.iter().zip(masks) {
            self.set_quad(quad, &self.mask_buffer, rect);
        }
        for (quad, rect) in self.border_quads.iter().zip(borders) {
            self.set_quad(quad, &self.border_buffer, rect);
        }
    }

    fn show_full_mask(&self, full: RectI) {
        self.set_quad(&self.mask_quads[0], &self.mask_buffer, full);
        for quad in &self.mask_quads[1..] {
            self.hide_quad(quad);
        }
        for quad in &self.border_quads {
            self.hide_quad(quad);
        }
    }

    fn set_quad(&self, quad: &OverlayQuad, buffer: &wl_buffer::WlBuffer, rect: RectI) {
        if rect.width <= 0 || rect.height <= 0 {
            self.hide_quad(quad);
            return;
        }
        quad.subsurface.set_position(rect.x, rect.y);
        quad.viewport.set_destination(rect.width, rect.height);
        quad.surface.attach(Some(buffer), 0, 0);
        quad.surface.damage_buffer(0, 0, 1, 1);
        quad.surface.commit();
    }

    fn hide_quad(&self, quad: &OverlayQuad) {
        quad.surface.attach(None, 0, 0);
        quad.surface.commit();
    }

    fn destroy(self) {
        for quad in self.mask_quads.into_iter().chain(self.border_quads) {
            quad.viewport.destroy();
            quad.subsurface.destroy();
            quad.surface.destroy();
        }
        self.mask_buffer.destroy();
        self.border_buffer.destroy();
    }
}

fn selection_visual_rects(
    start: (f64, f64),
    end: (f64, f64),
    logical_size: PixelSize,
) -> Option<([RectI; 4], [RectI; 4])> {
    let width = i32::try_from(logical_size.width).unwrap_or(i32::MAX);
    let height = i32::try_from(logical_size.height).unwrap_or(i32::MAX);
    if width <= 0 || height <= 0 {
        return None;
    }
    let x1 = start.0.min(end.0).floor().clamp(0.0, f64::from(width)) as i32;
    let y1 = start.1.min(end.1).floor().clamp(0.0, f64::from(height)) as i32;
    let x2 = start.0.max(end.0).ceil().clamp(0.0, f64::from(width)) as i32;
    let y2 = start.1.max(end.1).ceil().clamp(0.0, f64::from(height)) as i32;
    if x2 <= x1 || y2 <= y1 {
        return None;
    }

    let selection_width = x2 - x1;
    let selection_height = y2 - y1;
    let masks = [
        RectI { x: 0, y: 0, width, height: y1 },
        RectI { x: 0, y: y2, width, height: height - y2 },
        RectI { x: 0, y: y1, width: x1, height: selection_height },
        RectI { x: x2, y: y1, width: width - x2, height: selection_height },
    ];
    let thickness = 2_i32.min(selection_width).min(selection_height).max(1);
    let borders = [
        RectI { x: x1, y: y1, width: selection_width, height: thickness },
        RectI { x: x1, y: y2 - thickness, width: selection_width, height: thickness },
        RectI { x: x1, y: y1, width: thickness, height: selection_height },
        RectI { x: x2 - thickness, y: y1, width: thickness, height: selection_height },
    ];
    Some((masks, borders))
}

pub(crate) fn run_connection(
    connection: Connection,
    frame: &CaptureFrame,
) -> Result<FrozenOverlayOutcome, CaptureError> {
    let display = connection.display();
    let mut event_queue = connection.new_event_queue::<RuntimeState>();
    let queue_handle = event_queue.handle();
    let _registry = display.get_registry(&queue_handle, ());
    let mut state = RuntimeState::default();

    bounded_roundtrip(
        &display,
        &queue_handle,
        &mut event_queue,
        &mut state,
        "Wayland overlay registry initialization",
    )?;
    bounded_roundtrip(&display, &queue_handle, &mut event_queue, &mut state, "Wayland overlay output initialization")?;

    let compositor = state
        .compositor
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("wl_compositor is not advertised by this compositor".into()))?;
    let subcompositor = state
        .subcompositor
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("wl_subcompositor is not advertised by this compositor".into()))?;
    let layer_shell = state
        .layer_shell
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("zwlr_layer_shell_v1 is not advertised by this compositor".into()))?;
    let viewporter = state
        .viewporter
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("wp_viewporter is not advertised by this compositor".into()))?;
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("wl_shm is not advertised by this compositor".into()))?;
    let output = select_output(&state, &frame.screen.id)?.proxy.clone();

    let surface = compositor.create_surface(&queue_handle, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        Some(&output),
        zwlr_layer_shell_v1::Layer::Overlay,
        "lisnap-selection".into(),
        &queue_handle,
        (),
    );
    layer_surface.set_size(0, 0);
    layer_surface.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Right
            | zwlr_layer_surface_v1::Anchor::Bottom
            | zwlr_layer_surface_v1::Anchor::Left,
    );
    layer_surface.set_exclusive_zone(-1);
    layer_surface.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    surface.commit();

    let (serial, logical_width, logical_height) =
        wait_for_configure(&mut event_queue, &mut state, Instant::now() + SETUP_TIMEOUT)?;
    if logical_width == 0 || logical_height == 0 {
        layer_surface.destroy();
        surface.destroy();
        return Err(CaptureError::InvalidFrame("Wayland layer-shell configured a zero-sized selection overlay".into()));
    }
    layer_surface.ack_configure(serial);
    state.logical_size = Some(PixelSize { width: logical_width, height: logical_height });

    let frame_size = frame.image.size();
    state.frame_size = Some(frame_size);
    let stride = frame_size
        .width
        .checked_mul(4)
        .ok_or_else(|| CaptureError::InvalidFrame("overlay row size overflow".into()))?;
    let byte_len = (stride as usize)
        .checked_mul(frame_size.height as usize)
        .ok_or_else(|| CaptureError::InvalidFrame("overlay SHM buffer size overflow".into()))?;
    let size_i32 = i32::try_from(byte_len)
        .map_err(|_| CaptureError::InvalidFrame("overlay SHM buffer exceeds protocol size".into()))?;
    let width_i32 = i32::try_from(frame_size.width)
        .map_err(|_| CaptureError::InvalidFrame("overlay width exceeds protocol range".into()))?;
    let height_i32 = i32::try_from(frame_size.height)
        .map_err(|_| CaptureError::InvalidFrame("overlay height exceeds protocol range".into()))?;
    let stride_i32 = i32::try_from(stride)
        .map_err(|_| CaptureError::InvalidFrame("overlay stride exceeds protocol range".into()))?;

    let native = rgba_to_xrgb(frame.image.pixels())?;
    let name = CString::new("lisnap-overlay")
        .map_err(|_| CaptureError::BackendFailure("invalid overlay memfd name".into()))?;
    let fd = memfd_create(name.as_c_str(), MemfdFlags::CLOEXEC)
        .map_err(|error| CaptureError::BackendFailure(format!("overlay memfd_create failed: {error}")))?;
    let mut file = File::from(fd);
    file.set_len(byte_len as u64)
        .map_err(|error| CaptureError::BackendFailure(format!("resizing overlay SHM buffer failed: {error}")))?;
    file.write_all(&native)
        .map_err(|error| CaptureError::BackendFailure(format!("writing overlay SHM buffer failed: {error}")))?;

    let pool = shm.create_pool(file.as_fd(), size_i32, &queue_handle, ());
    let buffer = pool.create_buffer(0, width_i32, height_i32, stride_i32, wl_shm::Format::Xrgb8888, &queue_handle, ());
    pool.destroy();

    let viewport = if frame_size.width != logical_width || frame_size.height != logical_height {
        let viewport = viewporter.get_viewport(&surface, &queue_handle, ());
        viewport.set_destination(
            i32::try_from(logical_width)
                .map_err(|_| CaptureError::InvalidFrame("overlay destination width exceeds protocol range".into()))?,
            i32::try_from(logical_height)
                .map_err(|_| CaptureError::InvalidFrame("overlay destination height exceeds protocol range".into()))?,
        );
        Some(viewport)
    } else {
        None
    };

    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width_i32, height_i32);
    surface.commit();

    state.visuals = Some(OverlayVisuals::new(
        &compositor,
        &subcompositor,
        &viewporter,
        &shm,
        &surface,
        PixelSize { width: logical_width, height: logical_height },
        &queue_handle,
    )?);
    event_queue
        .flush()
        .map_err(|error| CaptureError::BackendFailure(format!("mapping Wayland overlay failed: {error}")))?;

    let outcome = run_input_loop(&mut event_queue, &mut state);

    if let Some(visuals) = state.visuals.take() {
        visuals.destroy();
    }
    surface.attach(None, 0, 0);
    surface.commit();
    let _ = event_queue.roundtrip(&mut state);
    if let Some(viewport) = viewport {
        viewport.destroy();
    }
    buffer.destroy();
    layer_surface.destroy();
    surface.destroy();
    outcome
}

fn select_output<'a>(state: &'a RuntimeState, requested: &str) -> Result<&'a OutputRecord, CaptureError> {
    state
        .outputs
        .iter()
        .find(|(id, output)| output.name.as_deref() == Some(requested) || id.to_string() == requested)
        .map(|(_, output)| output)
        .ok_or_else(|| CaptureError::Unsupported(format!("Wayland output '{requested}' was not found for overlay")))
}

fn run_input_loop(
    event_queue: &mut wayland_client::EventQueue<RuntimeState>,
    state: &mut RuntimeState,
) -> Result<FrozenOverlayOutcome, CaptureError> {
    loop {
        if let Some(outcome) = state.outcome.take() {
            return Ok(outcome);
        }
        if state.closed {
            return Ok(FrozenOverlayOutcome::Closed);
        }
        event_queue
            .blocking_dispatch(state)
            .map_err(|error| CaptureError::BackendFailure(format!("Wayland overlay input dispatch failed: {error}")))?;
    }
}

fn pointer_selection(
    start: (f64, f64),
    end: (f64, f64),
    logical_size: PixelSize,
    frame_size: PixelSize,
) -> Option<PixelRect> {
    selection_to_pixels(
        LogicalRect { x: start.0, y: start.1, width: end.0 - start.0, height: end.1 - start.1 },
        LogicalRect { x: 0.0, y: 0.0, width: f64::from(logical_size.width), height: f64::from(logical_size.height) },
        frame_size,
    )
}

fn rgba_to_xrgb(rgba: &[u8]) -> Result<Vec<u8>, CaptureError> {
    if !rgba.len().is_multiple_of(4) {
        return Err(CaptureError::InvalidFrame("overlay RGBA byte length is not divisible by four".into()));
    }
    let mut native = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        native.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0]);
    }
    Ok(native)
}

fn wait_for_configure(
    event_queue: &mut wayland_client::EventQueue<RuntimeState>,
    state: &mut RuntimeState,
    deadline: Instant,
) -> Result<(u32, u32, u32), CaptureError> {
    loop {
        if state.closed {
            return Err(CaptureError::BackendFailure(
                "Wayland compositor closed the selection overlay before configure".into(),
            ));
        }
        if let Some(configure) = state.configure.take() {
            return Ok(configure);
        }
        dispatch_with_deadline(event_queue, state, deadline, "Wayland selection overlay configure")?;
    }
}

fn bounded_roundtrip(
    display: &wl_display::WlDisplay,
    queue_handle: &QueueHandle<RuntimeState>,
    event_queue: &mut wayland_client::EventQueue<RuntimeState>,
    state: &mut RuntimeState,
    context: &str,
) -> Result<(), CaptureError> {
    state.sync_done = false;
    let _callback = display.sync(queue_handle, ());
    let deadline = Instant::now() + SETUP_TIMEOUT;
    while !state.sync_done {
        dispatch_with_deadline(event_queue, state, deadline, context)?;
    }
    Ok(())
}

fn dispatch_with_deadline(
    event_queue: &mut wayland_client::EventQueue<RuntimeState>,
    state: &mut RuntimeState,
    deadline: Instant,
    context: &str,
) -> Result<(), CaptureError> {
    loop {
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
        let timeout = Timespec {
            tv_sec: remaining.as_secs().min(i64::MAX as u64) as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        };
        let connection_fd = read_guard.connection_fd();
        let mut poll_fds = [PollFd::from_borrowed_fd(connection_fd, PollFlags::IN | PollFlags::ERR | PollFlags::HUP)];
        match poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) => {
                drop(read_guard);
                return Err(CaptureError::Timeout);
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

impl Dispatch<wl_registry::WlRegistry, ()> for RuntimeState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(WL_COMPOSITOR_MAX_VERSION), queue_handle, ()));
            }
            wl_registry::Event::Global { name, interface, version } if interface == "wl_subcompositor" => {
                state.subcompositor = Some(registry.bind(name, version.min(1), queue_handle, ()));
            }
            wl_registry::Event::Global { name, interface, version } if interface == LAYER_SHELL => {
                state.layer_shell = Some(registry.bind(name, version.min(LAYER_SHELL_MAX_VERSION), queue_handle, ()));
            }
            wl_registry::Event::Global { name, interface, version } if interface == "wl_shm" => {
                state.shm = Some(registry.bind(name, version.min(1), queue_handle, ()));
            }
            wl_registry::Event::Global { name, interface, version } if interface == VIEWPORTER => {
                state.viewporter = Some(registry.bind(name, version.min(1), queue_handle, ()));
            }
            wl_registry::Event::Global { name, interface, version } if interface == "wl_seat" => {
                if state.seat.is_none() {
                    state.seat = Some(registry.bind(name, version.min(WL_SEAT_MAX_VERSION), queue_handle, ()));
                }
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

impl Dispatch<wl_callback::WlCallback, ()> for RuntimeState {
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

impl Dispatch<wl_compositor::WlCompositor, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_subcompositor::WlSubcompositor, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_subcompositor::WlSubcompositor,
        _event: wl_subcompositor::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_subsurface::WlSubsurface, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_subsurface::WlSubsurface,
        _event: wl_subsurface::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_region::WlRegion, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_region::WlRegion,
        _event: wl_region::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, OutputData> for RuntimeState {
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

impl Dispatch<wl_seat::WlSeat, ()> for RuntimeState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event {
            if capabilities.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(queue_handle, ()));
            }
            if capabilities.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(queue_handle, ()));
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for RuntimeState {
    fn event(
        state: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface_x, surface_y, .. }
            | wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                let current = (surface_x, surface_y);
                state.pointer_position = Some(current);
                if let (Some(anchor), Some(logical_size), Some(visuals)) =
                    (state.drag_anchor, state.logical_size, state.visuals.as_ref())
                {
                    visuals.update_selection(Some(anchor), Some(current), logical_size);
                    let _ = connection.flush();
                }
            }
            wl_pointer::Event::Button { button, state: WEnum::Value(button_state), .. } if button == BTN_LEFT => {
                match button_state {
                    wl_pointer::ButtonState::Pressed => {
                        state.drag_anchor = state.pointer_position;
                        if let (Some(current), Some(logical_size), Some(visuals)) =
                            (state.pointer_position, state.logical_size, state.visuals.as_ref())
                        {
                            visuals.update_selection(Some(current), Some(current), logical_size);
                            let _ = connection.flush();
                        }
                    }
                    wl_pointer::ButtonState::Released => {
                        if let (Some(anchor), Some(current), Some(logical_size), Some(frame_size)) =
                            (state.drag_anchor.take(), state.pointer_position, state.logical_size, state.frame_size)
                        {
                            if let Some(visuals) = state.visuals.as_ref() {
                                visuals.update_selection(Some(anchor), Some(current), logical_size);
                                let _ = connection.flush();
                            }
                            if let Some(selection) = pointer_selection(anchor, current, logical_size, frame_size) {
                                state.outcome = Some(FrozenOverlayOutcome::Selected(selection));
                            } else if let Some(visuals) = state.visuals.as_ref() {
                                visuals.update_selection(None, None, logical_size);
                                let _ = connection.flush();
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for RuntimeState {
    fn event(
        state: &mut Self,
        _keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key { key, state: WEnum::Value(key_state), .. } = event
            && key == ESC_KEY
            && matches!(key_state, wl_keyboard::KeyState::Pressed | wl_keyboard::KeyState::Repeated)
        {
            state.outcome = Some(FrozenOverlayOutcome::Cancelled);
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for RuntimeState {
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

impl Dispatch<wl_shm_pool::WlShmPool, ()> for RuntimeState {
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

impl Dispatch<wl_buffer::WlBuffer, ()> for RuntimeState {
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

impl Dispatch<wp_viewporter::WpViewporter, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &wp_viewporter::WpViewporter,
        _event: wp_viewporter::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_viewport::WpViewport, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &wp_viewport::WpViewport,
        _event: wp_viewport::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _event: zwlr_layer_shell_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for RuntimeState {
    fn event(
        state: &mut Self,
        _proxy: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                state.configure = Some((serial, width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.closed = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_is_converted_to_little_endian_xrgb() {
        assert_eq!(rgba_to_xrgb(&[10, 20, 30, 255, 40, 50, 60, 128]).unwrap(), vec![30, 20, 10, 0, 60, 50, 40, 0]);
    }

    #[test]
    fn pointer_selection_maps_surface_coordinates_to_pixels() {
        let selection = pointer_selection(
            (100.0, 50.0),
            (500.0, 250.0),
            PixelSize { width: 1000, height: 500 },
            PixelSize { width: 2000, height: 1000 },
        )
        .unwrap();
        assert_eq!(selection, PixelRect { x: 200, y: 100, width: 800, height: 400 });
    }

    #[test]
    fn selection_visuals_cover_outside_and_border_inside() {
        let (masks, borders) =
            selection_visual_rects((100.0, 50.0), (500.0, 250.0), PixelSize { width: 1000, height: 500 }).unwrap();
        assert_eq!(
            masks,
            [
                RectI { x: 0, y: 0, width: 1000, height: 50 },
                RectI { x: 0, y: 250, width: 1000, height: 250 },
                RectI { x: 0, y: 50, width: 100, height: 200 },
                RectI { x: 500, y: 50, width: 500, height: 200 },
            ]
        );
        assert_eq!(
            borders,
            [
                RectI { x: 100, y: 50, width: 400, height: 2 },
                RectI { x: 100, y: 248, width: 400, height: 2 },
                RectI { x: 100, y: 50, width: 2, height: 200 },
                RectI { x: 498, y: 50, width: 2, height: 200 },
            ]
        );
        assert_eq!(
            selection_visual_rects((500.0, 250.0), (100.0, 50.0), PixelSize { width: 1000, height: 500 },),
            Some((masks, borders))
        );
    }

    #[test]
    fn selection_visuals_reject_empty_drag() {
        assert_eq!(selection_visual_rects((100.0, 50.0), (100.0, 50.0), PixelSize { width: 1000, height: 500 },), None);
    }
}
