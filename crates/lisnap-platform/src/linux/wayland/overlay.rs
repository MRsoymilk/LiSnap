use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use lisnap_core::{CaptureError, PixelSize};
use rustix::{
    event::{PollFd, PollFlags, poll},
    time::Timespec,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::{wl_callback, wl_compositor, wl_display, wl_output, wl_registry, wl_surface},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const WL_COMPOSITOR_MAX_VERSION: u32 = 6;
const WL_OUTPUT_MAX_VERSION: u32 = 4;
const LAYER_SHELL_MAX_VERSION: u32 = 5;
const LAYER_SHELL: &str = "zwlr_layer_shell_v1";
const OVERLAY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OverlayLifecycle {
    pub(crate) output_id: String,
    pub(crate) configured_size: PixelSize,
    pub(crate) layer_shell_version: u32,
}

#[derive(Debug)]
struct OutputRecord {
    proxy: wl_output::WlOutput,
    name: Option<String>,
}

#[derive(Debug, Default)]
struct OverlayState {
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    layer_shell_version: Option<u32>,
    outputs: BTreeMap<u32, OutputRecord>,
    sync_done: bool,
    configure: Option<(u32, u32, u32)>,
    closed: bool,
}

#[derive(Debug)]
struct OutputData {
    global_name: u32,
}

pub(crate) fn lifecycle_connection(
    connection: Connection,
    output_id: Option<&str>,
) -> Result<OverlayLifecycle, CaptureError> {
    let display = connection.display();
    let mut event_queue = connection.new_event_queue::<OverlayState>();
    let queue_handle = event_queue.handle();
    let _registry = display.get_registry(&queue_handle, ());
    let mut state = OverlayState::default();

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
    let layer_shell = state
        .layer_shell
        .clone()
        .ok_or_else(|| CaptureError::Unsupported("zwlr_layer_shell_v1 is not advertised by this compositor".into()))?;
    let layer_shell_version = state.layer_shell_version.unwrap_or(1).min(LAYER_SHELL_MAX_VERSION);
    let (selected_id, output) = select_output(&state, output_id)?;
    let output_proxy = output.proxy.clone();
    let stable_id = output.name.clone().unwrap_or_else(|| selected_id.to_string());

    let surface = compositor.create_surface(&queue_handle, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        Some(&output_proxy),
        zwlr_layer_shell_v1::Layer::Overlay,
        "lisnap-overlay".into(),
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

    let lifecycle = wait_for_configure(&mut event_queue, &mut state, Instant::now() + OVERLAY_TIMEOUT).and_then(
        |(serial, width, height)| {
            if width == 0 || height == 0 {
                return Err(CaptureError::InvalidFrame("Wayland layer-shell configured a zero-sized overlay".into()));
            }
            layer_surface.ack_configure(serial);
            Ok(OverlayLifecycle {
                output_id: stable_id,
                configured_size: PixelSize { width, height },
                layer_shell_version,
            })
        },
    );

    layer_surface.destroy();
    surface.destroy();
    lifecycle
}

fn select_output<'a>(state: &'a OverlayState, requested: Option<&str>) -> Result<(u32, &'a OutputRecord), CaptureError> {
    if state.outputs.is_empty() {
        return Err(CaptureError::Unsupported(
            "Wayland compositor exposes no wl_output globals for overlay placement".into(),
        ));
    }
    if let Some(requested) = requested {
        return state
            .outputs
            .iter()
            .find(|(id, output)| output.name.as_deref() == Some(requested) || id.to_string() == requested)
            .map(|(id, output)| (*id, output))
            .ok_or_else(|| CaptureError::Unsupported(format!("Wayland output '{requested}' was not found")));
    }
    state
        .outputs
        .iter()
        .next()
        .map(|(id, output)| (*id, output))
        .ok_or_else(|| CaptureError::Unsupported("Wayland compositor exposes no output for overlay placement".into()))
}

fn wait_for_configure(
    event_queue: &mut wayland_client::EventQueue<OverlayState>,
    state: &mut OverlayState,
    deadline: Instant,
) -> Result<(u32, u32, u32), CaptureError> {
    loop {
        if state.closed {
            return Err(CaptureError::BackendFailure(
                "Wayland compositor closed the layer-shell overlay before configure".into(),
            ));
        }
        if let Some(configure) = state.configure.take() {
            return Ok(configure);
        }
        dispatch_with_deadline(event_queue, state, deadline, "Wayland layer-shell configure")?;
    }
}

fn bounded_roundtrip(
    display: &wl_display::WlDisplay,
    queue_handle: &QueueHandle<OverlayState>,
    event_queue: &mut wayland_client::EventQueue<OverlayState>,
    state: &mut OverlayState,
    context: &str,
) -> Result<(), CaptureError> {
    state.sync_done = false;
    let _callback = display.sync(queue_handle, ());
    let deadline = Instant::now() + OVERLAY_TIMEOUT;
    while !state.sync_done {
        dispatch_with_deadline(event_queue, state, deadline, context)?;
    }
    Ok(())
}

fn dispatch_with_deadline(
    event_queue: &mut wayland_client::EventQueue<OverlayState>,
    state: &mut OverlayState,
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

impl Dispatch<wl_registry::WlRegistry, ()> for OverlayState {
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
            wl_registry::Event::Global { name, interface, version } if interface == LAYER_SHELL => {
                state.layer_shell_version = Some(version);
                state.layer_shell = Some(registry.bind(name, version.min(LAYER_SHELL_MAX_VERSION), queue_handle, ()));
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

impl Dispatch<wl_callback::WlCallback, ()> for OverlayState {
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

impl Dispatch<wl_compositor::WlCompositor, ()> for OverlayState {
    fn event(
        _state: &mut Self,
        _compositor: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for OverlayState {
    fn event(
        _state: &mut Self,
        _surface: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, OutputData> for OverlayState {
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

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for OverlayState {
    fn event(
        _state: &mut Self,
        _shell: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _event: zwlr_layer_shell_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for OverlayState {
    fn event(
        state: &mut Self,
        _surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
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
    fn empty_output_inventory_is_rejected() {
        let state = OverlayState::default();
        assert!(matches!(select_output(&state, Some("DP-1")), Err(CaptureError::Unsupported(_))));
    }
}
