use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::{Seek, SeekFrom, Write},
    os::fd::AsFd,
    time::{Duration, Instant},
};

use lisnap_core::{CaptureError, CaptureFrame, LogicalRect, PixelRect, PixelSize, RgbaImage, selection_to_pixels};

use super::annotation::{
    Annotation, ArrowHandle, CONTEXT_MENU_SIZE, CUSTOM_DRAG_HUE, CUSTOM_DRAG_SV, CUSTOM_HUE_ID, CUSTOM_SV_ID,
    ContextMenu, ContextMenuKind, CustomColorEditor, DEFAULT_COLOR, DraftAnnotation, EditorTool, MosaicSource, PALETTE,
    RectI as AnnotationRect, SELECTION_DIMENSIONS_SIZE, SelectionDimensions, TextDraft, ToolbarAction,
    annotation_damage_regions, annotation_hit_test, arrow_handle_hit_test, color_menu_layout, compose_annotated_crop,
    context_menu_layout, custom_color_menu_layout, custom_hue_at, custom_sat_value_at, custom_slider_value,
    draft_damage_regions, key_to_ascii, normalized_rect, preload_system_font, render_annotations_patch_with_source,
    render_context_menu_patch, render_draft_patch_with_move, render_selection_dimensions_patch,
    render_toolbar_patch_with_color, selected_annotation_damage, selected_annotation_hit_test,
    selection_dimensions_layout, toolbar_layout, translate_annotation_in_selection,
};
use super::selection::{ResizeHandle, resize_handle_rects, resize_hit_test, resized_selection};
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
use wayland_protocols::wp::{
    cursor_shape::v1::client::{wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1},
    text_input::zv3::client::{zwp_text_input_manager_v3, zwp_text_input_v3},
    viewporter::client::{wp_viewport, wp_viewporter},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const WL_COMPOSITOR_MAX_VERSION: u32 = 6;
const WL_OUTPUT_MAX_VERSION: u32 = 4;
const WL_SEAT_MAX_VERSION: u32 = 9;
const LAYER_SHELL_MAX_VERSION: u32 = 5;
const LAYER_SHELL: &str = "zwlr_layer_shell_v1";
const VIEWPORTER: &str = "wp_viewporter";
const TEXT_INPUT_MANAGER_V3: &str = "zwp_text_input_manager_v3";
const CURSOR_SHAPE_MANAGER_V1: &str = "wp_cursor_shape_manager_v1";
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const ESC_KEY: u32 = 1;
const BACKSPACE_KEY: u32 = 14;
const ENTER_KEY: u32 = 28;
const LEFT_CTRL_KEY: u32 = 29;
const LEFT_SHIFT_KEY: u32 = 42;
const RIGHT_SHIFT_KEY: u32 = 54;
const RIGHT_CTRL_KEY: u32 = 97;
const Z_KEY: u32 = 44;
const Y_KEY: u32 = 21;
const A_KEY: u32 = 30;
const DELETE_KEY: u32 = 111;
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
const TEXT_CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrozenOverlayOutcome {
    Selected(PixelRect),
    Cancelled,
    Closed,
}

#[derive(Debug, Clone)]
pub(crate) enum EditorOverlayOutcome {
    Accepted { selection: PixelRect, image: RgbaImage, copy: bool },
    Cancelled,
    Closed,
}

#[derive(Debug, Clone)]
enum RuntimeOutcome {
    Selected(PixelRect),
    Accepted { selection: PixelRect, logical_size: PixelSize, annotations: Vec<Annotation>, copy: bool },
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
    parent: wl_surface::WlSurface,
    mask_quads: Vec<OverlayQuad>,
    border_quads: Vec<OverlayQuad>,
    handle_quads: Vec<OverlayQuad>,
    mask_buffer: wl_buffer::WlBuffer,
    border_buffer: wl_buffer::WlBuffer,
    dimensions_layer: EditorLayer,
    pending_dimensions: Option<SelectionDimensions>,
    _solid_file: File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorLayerKind {
    Dimensions,
    Static,
    Draft,
    Toolbar,
    Menu,
}

#[derive(Debug)]
struct CanvasBuffer {
    buffer: wl_buffer::WlBuffer,
    file: File,
    released: bool,
    content_regions: Vec<AnnotationRect>,
}

#[derive(Debug)]
struct EditorLayer {
    surface: wl_surface::WlSurface,
    subsurface: wl_subsurface::WlSubsurface,
    buffers: Vec<CanvasBuffer>,
    size: PixelSize,
    displayed_regions: Vec<AnnotationRect>,
}

struct EditorLayerFactory<'a> {
    compositor: &'a wl_compositor::WlCompositor,
    subcompositor: &'a wl_subcompositor::WlSubcompositor,
    shm: &'a wl_shm::WlShm,
    parent: &'a wl_surface::WlSurface,
    size: PixelSize,
    queue_handle: &'a QueueHandle<RuntimeState>,
}

struct DraftRenderContent<'a> {
    draft: Option<&'a DraftAnnotation>,
    text_draft: Option<&'a TextDraft>,
    cursor_visible: bool,
    moving: Option<&'a Annotation>,
    movable: Option<&'a Annotation>,
    source: Option<&'a MosaicSource>,
}

#[derive(Debug)]
struct EditorSurfaces {
    parent: wl_surface::WlSurface,
    static_layer: EditorLayer,
    draft_layer: EditorLayer,
    toolbar_layer: EditorLayer,
    menu_layer: EditorLayer,
}

#[derive(Debug, Clone, Copy)]
struct CanvasBufferData {
    layer: EditorLayerKind,
    slot: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnotationDragMode {
    Move,
    ArrowStart,
    ArrowEnd,
}

#[derive(Debug, Clone)]
struct MovingAnnotation {
    index: usize,
    before: Annotation,
    preview: Annotation,
    anchor: (f64, f64),
    mode: AnnotationDragMode,
}

#[derive(Debug, Clone)]
struct EditingExistingText {
    index: usize,
    before: Annotation,
}

#[derive(Debug, Clone, Copy)]
struct SelectionResize {
    handle: ResizeHandle,
    anchor: (f64, f64),
    before: AnnotationRect,
    before_pixels: PixelRect,
}

#[derive(Debug, Clone)]
enum EditOperation {
    Added { index: usize, annotation: Annotation },
    Changed { index: usize, before: Annotation, after: Annotation },
    Deleted { index: usize, annotation: Annotation },
    SelectionResized { before: (AnnotationRect, PixelRect), after: (AnnotationRect, PixelRect) },
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
    cursor_shape_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    cursor_shape_device: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    text_input_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    text_input_focus: bool,
    text_input_enabled: bool,
    text_input_commit_count: u32,
    ime_pending_preedit: Option<String>,
    ime_pending_commit: Option<String>,
    ime_pending_delete: Option<(u32, u32)>,
    outputs: BTreeMap<u32, OutputRecord>,
    sync_done: bool,
    configure: Option<(u32, u32, u32)>,
    logical_size: Option<PixelSize>,
    frame_size: Option<PixelSize>,
    pointer_position: Option<(f64, f64)>,
    pointer_enter_serial: Option<u32>,
    drag_anchor: Option<(f64, f64)>,
    outcome: Option<RuntimeOutcome>,
    visuals: Option<OverlayVisuals>,
    editor_enabled: bool,
    editing: bool,
    selection_logical: Option<AnnotationRect>,
    selection_pixels: Option<PixelRect>,
    resizing: Option<SelectionResize>,
    selection_visuals_dirty: bool,
    active_tool: EditorTool,
    annotations: Vec<Annotation>,
    selected_annotation: Option<usize>,
    last_text_click: Option<(usize, Instant)>,
    editing_existing_text: Option<EditingExistingText>,
    undo_operations: Vec<EditOperation>,
    redo_operations: Vec<EditOperation>,
    moving: Option<MovingAnnotation>,
    mosaic_source: Option<MosaicSource>,
    draft_annotation: Option<DraftAnnotation>,
    text_draft: Option<TextDraft>,
    stroke_width: i32,
    font_size: i32,
    mosaic_block_size: i32,
    color_index: usize,
    custom_color: [u8; 4],
    custom_color_editor: Option<CustomColorEditor>,
    context_menu: Option<ContextMenu>,
    menu_last_bounds: Option<AnnotationRect>,
    menu_dirty: bool,
    text_cursor_visible: bool,
    text_cursor_blink_at: Option<Instant>,
    editor_surfaces: Option<EditorSurfaces>,
    static_dirty: bool,
    draft_dirty: bool,
    toolbar_dirty: bool,
    shift_pressed: bool,
    ctrl_pressed: bool,
    runtime_error: Option<CaptureError>,
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
        let handle_quads: Vec<_> =
            (0..8).map(|_| Self::create_quad(compositor, subcompositor, viewporter, parent, queue_handle)).collect();

        let factory = EditorLayerFactory {
            compositor,
            subcompositor,
            shm,
            parent,
            size: SELECTION_DIMENSIONS_SIZE,
            queue_handle,
        };
        let dimensions_layer =
            EditorLayer::new(EditorLayerKind::Dimensions, &factory, handle_quads.last().map(|quad| &quad.surface))?;
        let visuals = Self {
            parent: parent.clone(),
            mask_quads,
            border_quads,
            handle_quads,
            mask_buffer,
            border_buffer,
            dimensions_layer,
            pending_dimensions: None,
            _solid_file: file,
        };
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
        subsurface.set_sync();
        let viewport = viewporter.get_viewport(&surface, queue_handle, ());
        OverlayQuad { surface, subsurface, viewport }
    }

    fn update_selection(&self, start: Option<(f64, f64)>, end: Option<(f64, f64)>, logical_size: PixelSize) {
        let width = i32::try_from(logical_size.width).unwrap_or(i32::MAX);
        let height = i32::try_from(logical_size.height).unwrap_or(i32::MAX);
        let full = RectI { x: 0, y: 0, width, height };
        let Some((start, end)) = start.zip(end) else {
            self.show_full_mask(full);
            self.parent.commit();
            return;
        };
        let Some((masks, borders)) = selection_visual_rects(start, end, logical_size) else {
            self.show_full_mask(full);
            self.parent.commit();
            return;
        };

        for (quad, rect) in self.mask_quads.iter().zip(masks) {
            self.set_quad(quad, &self.mask_buffer, rect);
        }
        for (quad, rect) in self.border_quads.iter().zip(borders) {
            self.set_quad(quad, &self.border_buffer, rect);
        }
        self.parent.commit();
    }

    fn show_full_mask(&self, full: RectI) {
        self.set_quad(&self.mask_quads[0], &self.mask_buffer, full);
        for quad in &self.mask_quads[1..] {
            self.hide_quad(quad);
        }
        for quad in &self.border_quads {
            self.hide_quad(quad);
        }
        for quad in &self.handle_quads {
            self.hide_quad(quad);
        }
    }

    fn update_resize_handles(&self, selection: Option<AnnotationRect>, screen: PixelSize) {
        if let Some(selection) = selection {
            for (quad, rect) in self.handle_quads.iter().zip(resize_handle_rects(selection, screen)) {
                self.set_quad(
                    quad,
                    &self.border_buffer,
                    RectI { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
                );
            }
        } else {
            for quad in &self.handle_quads {
                self.hide_quad(quad);
            }
        }
        self.parent.commit();
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

    fn top_surface(&self) -> &wl_surface::WlSurface {
        &self.dimensions_layer.surface
    }

    fn update_dimensions(&mut self, dimensions: Option<SelectionDimensions>) -> Result<(), CaptureError> {
        self.pending_dimensions = dimensions;
        if self.pending_dimensions.is_none() {
            self.dimensions_layer.surface.attach(None, 0, 0);
            self.dimensions_layer.surface.commit();
            self.parent.commit();
        }
        self.redraw_dimensions()
    }

    fn redraw_dimensions(&mut self) -> Result<(), CaptureError> {
        let Some(dimensions) = self.pending_dimensions.as_ref() else {
            return Ok(());
        };
        if !self.dimensions_layer.has_available_buffer() {
            return Ok(());
        }
        let pixels = render_selection_dimensions_patch(dimensions);
        let region = AnnotationRect { x: 0, y: 0, ..dimensions.bounds };
        if self.dimensions_layer.redraw_patch(region, &pixels)? {
            self.dimensions_layer.subsurface.set_position(dimensions.bounds.x, dimensions.bounds.y);
            self.parent.commit();
            self.pending_dimensions = None;
        }
        Ok(())
    }

    fn destroy(self) {
        self.dimensions_layer.destroy();
        for quad in self.mask_quads.into_iter().chain(self.border_quads).chain(self.handle_quads) {
            quad.viewport.destroy();
            quad.subsurface.destroy();
            quad.surface.destroy();
        }
        self.mask_buffer.destroy();
        self.border_buffer.destroy();
    }
}

impl EditorLayer {
    fn new(
        kind: EditorLayerKind,
        factory: &EditorLayerFactory<'_>,
        stack_above: Option<&wl_surface::WlSurface>,
    ) -> Result<Self, CaptureError> {
        let width = i32::try_from(factory.size.width)
            .map_err(|_| CaptureError::InvalidFrame("editor layer width exceeds protocol range".into()))?;
        let height = i32::try_from(factory.size.height)
            .map_err(|_| CaptureError::InvalidFrame("editor layer height exceeds protocol range".into()))?;
        let stride = factory
            .size
            .width
            .checked_mul(4)
            .ok_or_else(|| CaptureError::InvalidFrame("editor layer stride overflow".into()))?;
        let byte_len = (stride as usize)
            .checked_mul(factory.size.height as usize)
            .ok_or_else(|| CaptureError::InvalidFrame("editor layer buffer size overflow".into()))?;
        let byte_len_i32 = i32::try_from(byte_len)
            .map_err(|_| CaptureError::InvalidFrame("editor layer buffer exceeds protocol size".into()))?;
        let stride_i32 = i32::try_from(stride)
            .map_err(|_| CaptureError::InvalidFrame("editor layer stride exceeds protocol range".into()))?;

        let surface = factory.compositor.create_surface(factory.queue_handle, ());
        let empty_input = factory.compositor.create_region(factory.queue_handle, ());
        surface.set_input_region(Some(&empty_input));
        empty_input.destroy();
        let subsurface = factory.subcompositor.get_subsurface(&surface, factory.parent, factory.queue_handle, ());
        subsurface.set_position(0, 0);
        if let Some(sibling) = stack_above {
            subsurface.place_above(sibling);
        } else {
            subsurface.place_above(factory.parent);
        }
        subsurface.set_sync();

        let mut buffers = Vec::with_capacity(2);
        for slot in 0..2 {
            let name = CString::new(format!("lisnap-editor-{kind:?}-{slot}"))
                .map_err(|_| CaptureError::BackendFailure("invalid editor layer memfd name".into()))?;
            let fd = memfd_create(name.as_c_str(), MemfdFlags::CLOEXEC)
                .map_err(|error| CaptureError::BackendFailure(format!("editor memfd_create failed: {error}")))?;
            let file = File::from(fd);
            file.set_len(byte_len as u64)
                .map_err(|error| CaptureError::BackendFailure(format!("resizing editor SHM failed: {error}")))?;
            let pool = factory.shm.create_pool(file.as_fd(), byte_len_i32, factory.queue_handle, ());
            let buffer = pool.create_buffer(
                0,
                width,
                height,
                stride_i32,
                wl_shm::Format::Argb8888,
                factory.queue_handle,
                CanvasBufferData { layer: kind, slot },
            );
            pool.destroy();
            buffers.push(CanvasBuffer { buffer, file, released: true, content_regions: Vec::new() });
        }

        Ok(Self { surface, subsurface, buffers, size: factory.size, displayed_regions: Vec::new() })
    }

    fn has_available_buffer(&self) -> bool {
        self.buffers.iter().any(|buffer| buffer.released)
    }

    fn redraw_patch(&mut self, region: AnnotationRect, pixels: &[u8]) -> Result<bool, CaptureError> {
        let Some(slot) = self.buffers.iter().position(|buffer| buffer.released) else {
            return Ok(false);
        };
        let previous_displayed = self.displayed_regions.clone();
        let previous_buffer_regions = self.buffers[slot].content_regions.clone();
        {
            let buffer = &mut self.buffers[slot];
            for old in &previous_buffer_regions {
                if *old != region {
                    let clear = vec![0; old.width as usize * old.height as usize * 4];
                    write_patch(&mut buffer.file, self.size, *old, &clear)?;
                }
            }
            write_patch(&mut buffer.file, self.size, region, pixels)?;
            buffer.content_regions = vec![region];
            buffer.released = false;
            self.surface.attach(Some(&buffer.buffer), 0, 0);
        }
        for damage in previous_displayed.into_iter().chain(previous_buffer_regions).chain(std::iter::once(region)) {
            self.surface.damage_buffer(damage.x, damage.y, damage.width, damage.height);
        }
        self.surface.commit();
        self.displayed_regions = vec![region];
        Ok(true)
    }

    fn redraw_draft(
        &mut self,
        current_regions: &[AnnotationRect],
        content: DraftRenderContent<'_>,
    ) -> Result<bool, CaptureError> {
        let Some(slot) = self.buffers.iter().position(|buffer| buffer.released) else {
            return Ok(false);
        };
        let previous_displayed = self.displayed_regions.clone();
        let previous_buffer_regions = self.buffers[slot].content_regions.clone();
        let mut write_regions = previous_buffer_regions.clone();
        // Double-buffering can leave an old one-time marker in either slot.
        for region in previous_displayed.iter().chain(current_regions) {
            if !write_regions.contains(region) {
                write_regions.push(*region);
            }
        }

        {
            let buffer = &mut self.buffers[slot];
            for region in &write_regions {
                let pixels = render_draft_patch_with_move(
                    *region,
                    content.draft,
                    content.text_draft,
                    content.cursor_visible,
                    content.moving,
                    content.movable,
                    content.source,
                );
                write_patch(&mut buffer.file, self.size, *region, &pixels)?;
            }
            buffer.content_regions = current_regions.to_vec();
            buffer.released = false;
            self.surface.attach(Some(&buffer.buffer), 0, 0);
        }

        let mut damage_regions = previous_displayed;
        for region in previous_buffer_regions.iter().chain(current_regions) {
            if !damage_regions.contains(region) {
                damage_regions.push(*region);
            }
        }
        for damage in damage_regions {
            self.surface.damage_buffer(damage.x, damage.y, damage.width, damage.height);
        }
        self.surface.commit();
        self.displayed_regions = current_regions.to_vec();
        Ok(true)
    }

    fn mark_released(&mut self, slot: usize) {
        if let Some(buffer) = self.buffers.get_mut(slot) {
            buffer.released = true;
        }
    }

    fn destroy(self) {
        for buffer in self.buffers {
            buffer.buffer.destroy();
        }
        self.subsurface.destroy();
        self.surface.destroy();
    }
}

impl EditorSurfaces {
    fn new(
        compositor: &wl_compositor::WlCompositor,
        subcompositor: &wl_subcompositor::WlSubcompositor,
        shm: &wl_shm::WlShm,
        parent: &wl_surface::WlSurface,
        stack_above: Option<&wl_surface::WlSurface>,
        size: PixelSize,
        queue_handle: &QueueHandle<RuntimeState>,
    ) -> Result<Self, CaptureError> {
        let factory = EditorLayerFactory { compositor, subcompositor, shm, parent, size, queue_handle };
        let static_layer = EditorLayer::new(EditorLayerKind::Static, &factory, stack_above)?;
        let draft_layer = EditorLayer::new(EditorLayerKind::Draft, &factory, Some(&static_layer.surface))?;
        let toolbar_layer = EditorLayer::new(EditorLayerKind::Toolbar, &factory, Some(&draft_layer.surface))?;
        let menu_factory = EditorLayerFactory { size: CONTEXT_MENU_SIZE, ..factory };
        let menu_layer = EditorLayer::new(EditorLayerKind::Menu, &menu_factory, Some(&toolbar_layer.surface))?;
        parent.commit();
        Ok(Self { parent: parent.clone(), static_layer, draft_layer, toolbar_layer, menu_layer })
    }

    fn mark_released(&mut self, layer: EditorLayerKind, slot: usize) {
        match layer {
            EditorLayerKind::Dimensions => {}
            EditorLayerKind::Static => self.static_layer.mark_released(slot),
            EditorLayerKind::Draft => self.draft_layer.mark_released(slot),
            EditorLayerKind::Toolbar => self.toolbar_layer.mark_released(slot),
            EditorLayerKind::Menu => self.menu_layer.mark_released(slot),
        }
    }

    fn commit_parent(&self) {
        self.parent.commit();
    }

    fn destroy(self) {
        self.menu_layer.destroy();
        self.toolbar_layer.destroy();
        self.draft_layer.destroy();
        self.static_layer.destroy();
    }
}

fn write_patch(
    file: &mut File,
    surface_size: PixelSize,
    region: AnnotationRect,
    pixels: &[u8],
) -> Result<(), CaptureError> {
    if region.x < 0 || region.y < 0 || region.width <= 0 || region.height <= 0 {
        return Err(CaptureError::InvalidFrame("editor patch has invalid bounds".into()));
    }
    let x2 = u32::try_from(region.x + region.width)
        .map_err(|_| CaptureError::InvalidFrame("editor patch x range exceeds protocol range".into()))?;
    let y2 = u32::try_from(region.y + region.height)
        .map_err(|_| CaptureError::InvalidFrame("editor patch y range exceeds protocol range".into()))?;
    if x2 > surface_size.width || y2 > surface_size.height {
        return Err(CaptureError::InvalidFrame("editor patch exceeds surface bounds".into()));
    }
    let row_bytes = usize::try_from(region.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| CaptureError::InvalidFrame("editor patch row size overflow".into()))?;
    let expected = row_bytes
        .checked_mul(region.height as usize)
        .ok_or_else(|| CaptureError::InvalidFrame("editor patch size overflow".into()))?;
    if pixels.len() != expected {
        return Err(CaptureError::InvalidFrame("editor patch pixel length mismatch".into()));
    }

    for row in 0..region.height as usize {
        let y = region.y as u64 + row as u64;
        let offset = (y * u64::from(surface_size.width) + region.x as u64)
            .checked_mul(4)
            .ok_or_else(|| CaptureError::InvalidFrame("editor patch file offset overflow".into()))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| CaptureError::BackendFailure(format!("seeking editor SHM failed: {error}")))?;
        let start = row * row_bytes;
        file.write_all(&pixels[start..start + row_bytes])
            .map_err(|error| CaptureError::BackendFailure(format!("writing editor SHM patch failed: {error}")))?;
    }
    Ok(())
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
    match run_connection_mode(connection, frame, false)? {
        RuntimeOutcome::Selected(selection) => Ok(FrozenOverlayOutcome::Selected(selection)),
        RuntimeOutcome::Cancelled => Ok(FrozenOverlayOutcome::Cancelled),
        RuntimeOutcome::Closed => Ok(FrozenOverlayOutcome::Closed),
        RuntimeOutcome::Accepted { .. } => {
            Err(CaptureError::BackendFailure("selection overlay unexpectedly returned an editor result".into()))
        }
    }
}

pub(crate) fn run_editor_connection(
    connection: Connection,
    frame: &CaptureFrame,
) -> Result<EditorOverlayOutcome, CaptureError> {
    match run_connection_mode(connection, frame, true)? {
        RuntimeOutcome::Accepted { selection, logical_size, annotations, copy } => Ok(EditorOverlayOutcome::Accepted {
            selection,
            image: compose_annotated_crop(frame, logical_size, selection, &annotations)?,
            copy,
        }),
        RuntimeOutcome::Cancelled => Ok(EditorOverlayOutcome::Cancelled),
        RuntimeOutcome::Closed => Ok(EditorOverlayOutcome::Closed),
        RuntimeOutcome::Selected(_) => {
            Err(CaptureError::BackendFailure("editor overlay unexpectedly returned a selection-only result".into()))
        }
    }
}

fn run_connection_mode(
    connection: Connection,
    frame: &CaptureFrame,
    editor_enabled: bool,
) -> Result<RuntimeOutcome, CaptureError> {
    if editor_enabled {
        preload_system_font();
    }
    let display = connection.display();
    let mut event_queue = connection.new_event_queue::<RuntimeState>();
    let queue_handle = event_queue.handle();
    let _registry = display.get_registry(&queue_handle, ());
    let mut state = RuntimeState {
        editor_enabled,
        stroke_width: 3,
        font_size: 24,
        mosaic_block_size: 18,
        custom_color: DEFAULT_COLOR,
        ..RuntimeState::default()
    };

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
    require_input_capabilities(state.pointer.is_some(), state.keyboard.is_some())?;
    if editor_enabled && let (Some(manager), Some(seat)) = (state.text_input_manager.as_ref(), state.seat.as_ref()) {
        state.text_input = Some(manager.get_text_input(seat, &queue_handle, ()));
    }
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
    if editor_enabled {
        state.mosaic_source = Some(MosaicSource {
            image: frame.image.clone(),
            logical_size: PixelSize { width: logical_width, height: logical_height },
        });
    }

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
    if editor_enabled {
        let stack_above = state.visuals.as_ref().map(|visuals| visuals.top_surface().clone());
        state.editor_surfaces = Some(EditorSurfaces::new(
            &compositor,
            &subcompositor,
            &shm,
            &surface,
            stack_above.as_ref(),
            PixelSize { width: logical_width, height: logical_height },
            &queue_handle,
        )?);
    }
    event_queue
        .flush()
        .map_err(|error| CaptureError::BackendFailure(format!("mapping Wayland overlay failed: {error}")))?;

    let outcome = run_input_loop(&mut event_queue, &mut state, &connection);

    if let Some(text_input) = state.text_input.take() {
        if state.text_input_enabled {
            text_input.disable();
            text_input.commit();
        }
        text_input.destroy();
    }
    if let Some(editor_surfaces) = state.editor_surfaces.take() {
        editor_surfaces.destroy();
    }
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
    connection: &Connection,
) -> Result<RuntimeOutcome, CaptureError> {
    loop {
        if let Some(error) = state.runtime_error.take() {
            return Err(error);
        }
        if let Some(outcome) = state.outcome.take() {
            return Ok(outcome);
        }
        if state.closed {
            return Ok(RuntimeOutcome::Closed);
        }

        if let Some(deadline) = state.text_cursor_blink_at {
            match dispatch_with_deadline(event_queue, state, deadline, "Wayland overlay input dispatch") {
                Ok(()) => {}
                Err(CaptureError::Timeout) => {
                    if state.text_draft.is_some() {
                        state.text_cursor_visible = !state.text_cursor_visible;
                        state.text_cursor_blink_at = Some(Instant::now() + TEXT_CURSOR_BLINK_INTERVAL);
                        state.draft_dirty = true;
                        redraw_editor(state, connection);
                    } else {
                        state.text_cursor_visible = false;
                        state.text_cursor_blink_at = None;
                    }
                }
                Err(error) => return Err(error),
            }
        } else {
            event_queue.blocking_dispatch(state).map_err(|error| {
                CaptureError::BackendFailure(format!("Wayland overlay input dispatch failed: {error}"))
            })?;
        }
    }
}

fn reset_text_cursor_blink(state: &mut RuntimeState) {
    if state.text_draft.is_some() {
        state.text_cursor_visible = true;
        state.text_cursor_blink_at = Some(Instant::now() + TEXT_CURSOR_BLINK_INTERVAL);
        state.draft_dirty = true;
    } else {
        state.text_cursor_visible = false;
        state.text_cursor_blink_at = None;
    }
}

fn stop_text_cursor_blink(state: &mut RuntimeState) {
    state.text_cursor_visible = false;
    state.text_cursor_blink_at = None;
}

fn require_input_capabilities(pointer: bool, keyboard: bool) -> Result<(), CaptureError> {
    match (pointer, keyboard) {
        (true, true) => Ok(()),
        (false, false) => Err(CaptureError::Unsupported(
            "Wayland selection overlay requires pointer and keyboard seat capabilities".into(),
        )),
        (false, true) => {
            Err(CaptureError::Unsupported("Wayland selection overlay requires a pointer seat capability".into()))
        }
        (true, false) => Err(CaptureError::Unsupported(
            "Wayland selection overlay requires a keyboard seat capability for Esc cancellation".into(),
        )),
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

fn selection_dimensions(
    start: (f64, f64),
    end: (f64, f64),
    logical_size: PixelSize,
    frame_size: PixelSize,
) -> Option<SelectionDimensions> {
    let selection = normalized_rect(start, end, logical_size)?;
    let pixels = pointer_selection(start, end, logical_size, frame_size)?;
    Some(selection_dimensions_layout(selection, pixels, logical_size))
}

fn update_selection_visuals(
    state: &mut RuntimeState,
    start: Option<(f64, f64)>,
    end: Option<(f64, f64)>,
    connection: &Connection,
) {
    let (Some(logical_size), Some(frame_size), Some(visuals)) =
        (state.logical_size, state.frame_size, state.visuals.as_mut())
    else {
        return;
    };
    visuals.update_selection(start, end, logical_size);
    visuals.update_resize_handles(if state.editing { state.selection_logical } else { None }, logical_size);
    let dimensions = if state.editing {
        state
            .selection_logical
            .zip(state.selection_pixels)
            .map(|(selection, pixels)| selection_dimensions_layout(selection, pixels, logical_size))
    } else {
        start.zip(end).and_then(|(start, end)| selection_dimensions(start, end, logical_size, frame_size))
    };
    if let Err(error) = visuals.update_dimensions(dimensions) {
        state.runtime_error = Some(error);
    }
    let _ = connection.flush();
}

fn commit_text_input_request(state: &mut RuntimeState, text_input: &zwp_text_input_v3::ZwpTextInputV3) {
    text_input.commit();
    state.text_input_commit_count = state.text_input_commit_count.wrapping_add(1);
}

fn send_text_input_state(state: &mut RuntimeState, text_input: &zwp_text_input_v3::ZwpTextInputV3) {
    let Some(text) = state.text_draft.as_ref() else {
        return;
    };
    let cursor = i32::try_from(text.text.len()).unwrap_or(i32::MAX);
    text_input.set_surrounding_text(text.text.clone(), cursor, if text.selected_all { 0 } else { cursor });
    text_input
        .set_content_type(zwp_text_input_v3::ContentHint::PreeditShown, zwp_text_input_v3::ContentPurpose::Normal);
    let cursor_x = text.position.0.round() as i32
        + i32::try_from(text.text.chars().count()).unwrap_or(i32::MAX).saturating_mul(18);
    let cursor_y = text.position.1.round() as i32;
    text_input.set_cursor_rectangle(cursor_x, cursor_y, 2, 28);
    commit_text_input_request(state, text_input);
}

fn update_text_input_activation(state: &mut RuntimeState, connection: &Connection) {
    let desired = state.text_draft.is_some()
        && state.text_input_focus
        && !state.context_menu.as_ref().is_some_and(|menu| menu.kind == ContextMenuKind::CustomColor);
    if desired == state.text_input_enabled {
        return;
    }
    let Some(text_input) = state.text_input.clone() else {
        return;
    };
    if desired {
        text_input.enable();
        state.text_input_enabled = true;
        send_text_input_state(state, &text_input);
    } else {
        text_input.disable();
        commit_text_input_request(state, &text_input);
        state.text_input_enabled = false;
        state.ime_pending_preedit = None;
        state.ime_pending_commit = None;
        state.ime_pending_delete = None;
        if let Some(text) = state.text_draft.as_mut()
            && !text.preedit.is_empty()
        {
            text.preedit.clear();
            state.draft_dirty = true;
        }
    }
    if let Err(error) = connection.flush() {
        state.runtime_error = Some(CaptureError::BackendFailure(format!("flushing text-input state failed: {error}")));
    }
}

fn delete_utf8_suffix_bytes(text: &mut String, before_length: u32) {
    let remove = usize::try_from(before_length).unwrap_or(usize::MAX);
    let mut cut = text.len().saturating_sub(remove);
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
}

fn apply_ime_text_change(text: &mut TextDraft, delete: Option<(u32, u32)>, commit: &str) -> bool {
    if delete.is_none() && commit.is_empty() {
        return false;
    }
    let mut updated = if text.selected_all { String::new() } else { text.text.clone() };
    if let Some((before_length, _after_length)) = delete {
        delete_utf8_suffix_bytes(&mut updated, before_length);
    }
    updated.push_str(commit);
    text.update_text(updated)
}

fn apply_text_input_done(state: &mut RuntimeState, serial: u32, connection: &Connection) {
    let preedit = state.ime_pending_preedit.take().unwrap_or_default();
    let commit = state.ime_pending_commit.take().unwrap_or_default();
    let delete = state.ime_pending_delete.take();
    let mut surrounding_changed = false;
    if let Some(text) = state.text_draft.as_mut() {
        surrounding_changed = apply_ime_text_change(text, delete, &commit);
        if text.preedit != preedit {
            text.preedit = preedit;
            state.draft_dirty = true;
        }
        state.draft_dirty |= surrounding_changed;
    }
    if state.text_draft.is_some() {
        reset_text_cursor_blink(state);
    }
    if surrounding_changed
        && serial == state.text_input_commit_count
        && let Some(text_input) = state.text_input.clone()
        && state.text_input_enabled
    {
        send_text_input_state(state, &text_input);
    }
    redraw_editor(state, connection);
}

fn redraw_editor(state: &mut RuntimeState, connection: &Connection) {
    if !state.editor_enabled || !state.editing {
        return;
    }
    if state.selection_visuals_dirty {
        if let Some(selection) = state.selection_logical {
            let (start, end) = selection_corners(selection);
            update_selection_visuals(state, Some(start), Some(end), connection);
        }
        state.selection_visuals_dirty = false;
    }
    update_text_input_activation(state, connection);
    let Some(selection) = state.selection_logical else {
        return;
    };
    let Some(logical_size) = state.logical_size else {
        return;
    };
    let Some(mut surfaces) = state.editor_surfaces.take() else {
        return;
    };

    let mut parent_dirty = false;
    if state.static_dirty && surfaces.static_layer.has_available_buffer() {
        let pixels = render_annotations_patch_with_source(selection, &state.annotations, state.mosaic_source.as_ref());
        match surfaces.static_layer.redraw_patch(selection, &pixels) {
            Ok(true) => {
                state.static_dirty = false;
                parent_dirty = true;
            }
            Ok(false) => {}
            Err(error) => state.runtime_error = Some(error),
        }
    }

    if state.draft_dirty && state.runtime_error.is_none() {
        let mut regions = draft_damage_regions(state.draft_annotation.as_ref(), state.text_draft.as_ref(), selection);
        if let Some(moving) = state.moving.as_ref() {
            regions.extend(annotation_damage_regions(&moving.preview, selection));
            regions.extend(selected_annotation_damage(&moving.preview, selection));
        }
        let editing_text_selection = state.editing_existing_text.as_ref().and_then(|_| {
            state.text_draft.as_ref().map(|text| Annotation::Text {
                position: text.position,
                text: format!("{}{}", text.text, text.preedit),
                font_size: text.font_size,
                color: text.color,
            })
        });
        let selected = state
            .selected_annotation
            .and_then(|index| state.annotations.get(index))
            .or(editing_text_selection.as_ref());
        if let Some(annotation) = selected {
            regions.extend(selected_annotation_damage(annotation, selection));
        }
        let content = DraftRenderContent {
            draft: state.draft_annotation.as_ref(),
            text_draft: state.text_draft.as_ref(),
            cursor_visible: state.text_cursor_visible,
            moving: state.moving.as_ref().map(|moving| &moving.preview),
            movable: selected,
            source: state.mosaic_source.as_ref(),
        };
        match surfaces.draft_layer.redraw_draft(&regions, content) {
            Ok(true) => {
                state.draft_dirty = false;
                parent_dirty = true;
            }
            Ok(false) => {}
            Err(error) => state.runtime_error = Some(error),
        }
    }

    if state.toolbar_dirty && state.runtime_error.is_none() && surfaces.toolbar_layer.has_available_buffer() {
        let toolbar = toolbar_layout(selection, logical_size);
        let color = active_annotation_color(state);
        let pixels = render_toolbar_patch_with_color(&toolbar, state.active_tool, color);
        match surfaces.toolbar_layer.redraw_patch(toolbar.bounds, &pixels) {
            Ok(true) => {
                state.toolbar_dirty = false;
                parent_dirty = true;
            }
            Ok(false) => {}
            Err(error) => state.runtime_error = Some(error),
        }
    }

    // Never generate a full color picker frame while both Wayland SHM slots
    // are in use. Keep menu_dirty set and render the latest value on wl_buffer.release.
    if state.menu_dirty && state.runtime_error.is_none() && surfaces.menu_layer.has_available_buffer() {
        let bounds = state.context_menu.as_ref().map(|menu| menu.bounds).or(state.menu_last_bounds);
        if let Some(bounds) = bounds {
            if state.context_menu.is_some() {
                surfaces.menu_layer.subsurface.set_position(bounds.x, bounds.y);
            }
            let pixels = if let Some(menu) = state.context_menu.as_ref() {
                let selected = match menu.kind {
                    ContextMenuKind::Stroke => state.stroke_width,
                    ContextMenuKind::Font => state.font_size,
                    ContextMenuKind::Color => state.color_index as i32,
                    ContextMenuKind::CustomColor => PALETTE.len() as i32,
                    ContextMenuKind::Mosaic => state.mosaic_block_size,
                };
                render_context_menu_patch(menu, selected, state.custom_color, state.custom_color_editor.as_ref())
            } else {
                vec![0; CONTEXT_MENU_SIZE.width as usize * CONTEXT_MENU_SIZE.height as usize * 4]
            };
            let local_bounds = AnnotationRect {
                x: 0,
                y: 0,
                width: CONTEXT_MENU_SIZE.width as i32,
                height: CONTEXT_MENU_SIZE.height as i32,
            };
            match surfaces.menu_layer.redraw_patch(local_bounds, &pixels) {
                Ok(true) => {
                    state.menu_last_bounds = state.context_menu.as_ref().map(|menu| menu.bounds);
                    state.menu_dirty = false;
                    parent_dirty = true;
                }
                Ok(false) => {}
                Err(error) => state.runtime_error = Some(error),
            }
        } else {
            state.menu_dirty = false;
        }
    }

    if parent_dirty {
        surfaces.commit_parent();
        if let Err(error) = connection.flush() {
            state.runtime_error =
                Some(CaptureError::BackendFailure(format!("flushing editor overlay failed: {error}")));
        }
    }
    state.editor_surfaces = Some(surfaces);
}

fn add_annotation(state: &mut RuntimeState, annotation: Annotation) {
    let index = state.annotations.len();
    state.annotations.push(annotation.clone());
    state.selected_annotation = Some(index);
    state.last_text_click = None;
    state.undo_operations.push(EditOperation::Added { index, annotation });
    state.redo_operations.clear();
    state.static_dirty = true;
    state.draft_dirty = true;
}

fn begin_existing_text_edit(state: &mut RuntimeState, index: usize) -> bool {
    let Some(Annotation::Text { position, text, font_size, color }) = state.annotations.get(index).cloned() else {
        return false;
    };
    let mut draft = TextDraft::with_font_size(position, font_size);
    draft.text = text;
    draft.color = color;
    let before = state.annotations.remove(index);
    state.editing_existing_text = Some(EditingExistingText { index, before });
    state.text_draft = Some(draft);
    state.selected_annotation = None;
    state.last_text_click = None;
    state.active_tool = EditorTool::Text;
    state.toolbar_dirty = true;
    state.static_dirty = true;
    reset_text_cursor_blink(state);
    true
}

fn commit_text_draft(state: &mut RuntimeState) {
    let Some(text) = state.text_draft.take() else {
        return;
    };
    stop_text_cursor_blink(state);
    state.draft_dirty = true;
    if let Some(existing) = state.editing_existing_text.take() {
        let index = existing.index.min(state.annotations.len());
        if text.text.is_empty() {
            state.undo_operations.push(EditOperation::Deleted { index, annotation: existing.before });
            state.redo_operations.clear();
            state.selected_annotation = None;
        } else {
            let after = Annotation::Text {
                position: text.position,
                text: text.text,
                font_size: text.font_size,
                color: text.color,
            };
            if existing.before != after {
                state.undo_operations.push(EditOperation::Changed {
                    index,
                    before: existing.before,
                    after: after.clone(),
                });
                state.redo_operations.clear();
            }
            state.annotations.insert(index, after);
            state.selected_annotation = Some(index);
        }
        state.static_dirty = true;
    } else if !text.text.is_empty() {
        add_annotation(
            state,
            Annotation::Text { position: text.position, text: text.text, font_size: text.font_size, color: text.color },
        );
    }
}

fn cancel_text_draft(state: &mut RuntimeState) {
    state.text_draft = None;
    stop_text_cursor_blink(state);
    if let Some(existing) = state.editing_existing_text.take() {
        let index = existing.index.min(state.annotations.len());
        state.annotations.insert(index, existing.before);
        state.selected_annotation = Some(index);
        state.static_dirty = true;
    }
    state.draft_dirty = true;
}

fn apply_edit_operation(state: &mut RuntimeState, op: &EditOperation, forward: bool) {
    match op {
        EditOperation::Added { index, annotation } => {
            if forward {
                state.annotations.insert((*index).min(state.annotations.len()), annotation.clone());
            } else if *index < state.annotations.len() && state.annotations[*index] == *annotation {
                state.annotations.remove(*index);
            }
        }
        EditOperation::Changed { index, before, after } => {
            if let Some(annotation) = state.annotations.get_mut(*index) {
                *annotation = if forward { after.clone() } else { before.clone() };
            }
        }
        EditOperation::Deleted { index, annotation } => {
            if forward {
                if *index < state.annotations.len() && state.annotations[*index] == *annotation {
                    state.annotations.remove(*index);
                }
            } else {
                state.annotations.insert((*index).min(state.annotations.len()), annotation.clone());
            }
        }
        EditOperation::SelectionResized { before, after } => {
            let (selection, pixels) = if forward { *after } else { *before };
            set_selection(state, selection, pixels);
        }
    }
    state.selected_annotation = None;
    state.last_text_click = None;
    state.static_dirty = true;
    state.draft_dirty = true;
}

fn undo_editor(state: &mut RuntimeState) {
    if let Some(text) = state.text_draft.as_mut() {
        if text.undo() {
            reset_text_cursor_blink(state);
        }
        return;
    }
    if state.selected_annotation.take().is_some() {
        state.draft_dirty = true;
    }
    if let Some(op) = state.undo_operations.pop() {
        apply_edit_operation(state, &op, false);
        state.redo_operations.push(op);
    } else if let Some(annotation) = state.annotations.pop() {
        // Legacy/imported entries without operation history.
        state.redo_operations.push(EditOperation::Added { index: state.annotations.len(), annotation });
        state.static_dirty = true;
    }
}

fn redo_editor(state: &mut RuntimeState) {
    if let Some(text) = state.text_draft.as_mut() {
        if text.redo() {
            reset_text_cursor_blink(state);
        }
        return;
    }
    if state.selected_annotation.take().is_some() {
        state.draft_dirty = true;
    }
    if let Some(op) = state.redo_operations.pop() {
        apply_edit_operation(state, &op, true);
        state.undo_operations.push(op);
    }
}

fn accept_editor(state: &mut RuntimeState, copy: bool) {
    commit_text_draft(state);
    if let (Some(selection), Some(logical_size)) = (state.selection_pixels, state.logical_size) {
        state.outcome =
            Some(RuntimeOutcome::Accepted { selection, logical_size, annotations: state.annotations.clone(), copy });
    }
}

fn handle_toolbar_action(state: &mut RuntimeState, action: ToolbarAction) {
    match action {
        ToolbarAction::Color => {}
        ToolbarAction::Tool(tool) => {
            commit_text_draft(state);
            state.selected_annotation = None;
            state.last_text_click = None;
            state.draft_dirty = true;
            state.active_tool = tool;
            if state.draft_annotation.take().is_some() {
                state.draft_dirty = true;
            }
            state.toolbar_dirty = true;
        }
        ToolbarAction::Undo => undo_editor(state),
        ToolbarAction::Redo => redo_editor(state),
        ToolbarAction::Confirm | ToolbarAction::Save => accept_editor(state, false),
        ToolbarAction::Copy => accept_editor(state, true),
        ToolbarAction::Cancel => state.outcome = Some(RuntimeOutcome::Cancelled),
    }
}

fn clamp_to_selection(point: (f64, f64), selection: AnnotationRect) -> (f64, f64) {
    (
        point.0.clamp(f64::from(selection.x), f64::from(selection.x + selection.width)),
        point.1.clamp(f64::from(selection.y), f64::from(selection.y + selection.height)),
    )
}

fn active_annotation_color(state: &RuntimeState) -> [u8; 4] {
    PALETTE.get(state.color_index).map_or(state.custom_color, |(_, color)| *color)
}

fn set_active_annotation_color(state: &mut RuntimeState, index: usize, color: [u8; 4]) {
    state.color_index = index;
    state.toolbar_dirty = true;
    if let Some(text) = state.text_draft.as_mut() {
        text.color = color;
        state.draft_dirty = true;
    }
    if let Some(draft) = state.draft_annotation.as_mut() {
        match draft {
            DraftAnnotation::Pen { color: current, .. }
            | DraftAnnotation::Rectangle { color: current, .. }
            | DraftAnnotation::Arrow { color: current, .. } => *current = color,
            DraftAnnotation::Mosaic { .. } => {}
        }
        state.draft_dirty = true;
    }
}

fn open_custom_color_editor(state: &mut RuntimeState) {
    let (Some(menu), Some(screen)) = (state.context_menu.as_ref(), state.logical_size) else {
        return;
    };
    let point = (f64::from(menu.bounds.x), f64::from(menu.bounds.y));
    let initial_color = active_annotation_color(state);
    state.custom_color_editor = Some(CustomColorEditor::new(initial_color));
    state.context_menu = Some(custom_color_menu_layout(point, screen));
    state.menu_dirty = true;
}

fn back_to_color_palette(state: &mut RuntimeState) {
    let (Some(menu), Some(screen)) = (state.context_menu.as_ref(), state.logical_size) else {
        close_editor_context_menu(state);
        return;
    };
    let point = (f64::from(menu.bounds.x), f64::from(menu.bounds.y));
    state.custom_color_editor = None;
    state.context_menu = Some(color_menu_layout(point, screen));
    state.menu_dirty = true;
}

fn apply_custom_color_editor(state: &mut RuntimeState) -> bool {
    let Some(color) = state.custom_color_editor.as_ref().and_then(CustomColorEditor::chosen_color) else {
        return false;
    };
    state.custom_color = color;
    set_active_annotation_color(state, PALETTE.len(), color);
    close_editor_context_menu(state);
    true
}

fn move_custom_color_slider(state: &mut RuntimeState, channel: usize, pointer_x: f64) -> bool {
    let Some(menu) = state.context_menu.as_ref() else { return false };
    let Some(value) = custom_slider_value(menu, channel, pointer_x) else { return false };
    let Some(editor) = state.custom_color_editor.as_mut() else { return false };
    let changed = editor.set_channel(channel, value);
    if changed {
        state.menu_dirty = true;
    }
    changed
}

fn move_custom_color_square(state: &mut RuntimeState, point: (f64, f64)) -> bool {
    let Some(menu) = state.context_menu.as_ref() else { return false };
    let Some((saturation, value)) = custom_sat_value_at(menu, point) else { return false };
    let Some(editor) = state.custom_color_editor.as_mut() else { return false };
    let changed = editor.set_saturation_value(saturation, value);
    if changed {
        state.menu_dirty = true;
    }
    changed
}

fn move_custom_color_hue(state: &mut RuntimeState, pointer_x: f64) -> bool {
    let Some(menu) = state.context_menu.as_ref() else { return false };
    let Some(hue) = custom_hue_at(menu, pointer_x) else { return false };
    let Some(editor) = state.custom_color_editor.as_mut() else { return false };
    let changed = editor.set_hue(hue);
    if changed {
        state.menu_dirty = true;
    }
    changed
}

fn press_custom_color_editor(state: &mut RuntimeState, point: (f64, f64)) {
    let Some(menu) = state.context_menu.as_ref() else { return };
    let hit = menu.hit_test(point);
    let inside = menu.bounds.contains(point);
    match hit {
        Some(0..=2) => {
            let channel = hit.unwrap() as usize;
            if let Some(editor) = state.custom_color_editor.as_mut() {
                editor.dragging_channel = Some(channel);
            }
            move_custom_color_slider(state, channel, point.0);
        }
        Some(CUSTOM_SV_ID) => {
            if let Some(editor) = state.custom_color_editor.as_mut() {
                editor.dragging_channel = Some(CUSTOM_DRAG_SV);
            }
            move_custom_color_square(state, point);
        }
        Some(CUSTOM_HUE_ID) => {
            if let Some(editor) = state.custom_color_editor.as_mut() {
                editor.dragging_channel = Some(CUSTOM_DRAG_HUE);
            }
            move_custom_color_hue(state, point.0);
        }
        Some(3) => {
            if let Some(editor) = state.custom_color_editor.as_mut() {
                editor.start_hex_input();
                state.menu_dirty = true;
            }
        }
        Some(4) => {
            apply_custom_color_editor(state);
        }
        Some(5) => back_to_color_palette(state),
        _ if !inside => close_editor_context_menu(state),
        _ => {}
    }
}

fn close_editor_context_menu(state: &mut RuntimeState) {
    state.custom_color_editor = None;
    if state.context_menu.take().is_some() {
        state.menu_dirty = true;
    }
}

fn begin_editor_context_menu(state: &mut RuntimeState, point: (f64, f64), connection: &Connection) {
    let (Some(selection), Some(screen)) = (state.selection_logical, state.logical_size) else {
        return;
    };
    let toolbar = toolbar_layout(selection, screen);
    let chosen = toolbar.hit_test(point);
    if chosen == Some(ToolbarAction::Color) {
        state.context_menu = Some(color_menu_layout(point, screen));
        state.menu_dirty = true;
        redraw_editor(state, connection);
        return;
    }
    let tool = if let Some(ToolbarAction::Tool(tool)) = chosen {
        if tool != state.active_tool {
            handle_toolbar_action(state, ToolbarAction::Tool(tool));
        }
        tool
    } else if chosen.is_some() {
        return;
    } else if selection.contains(point) {
        state.active_tool
    } else {
        close_editor_context_menu(state);
        redraw_editor(state, connection);
        return;
    };
    state.context_menu = Some(context_menu_layout(tool, point, screen));
    state.menu_dirty = true;
    state.drag_anchor = None;
    redraw_editor(state, connection);
}

fn apply_editor_context_choice(state: &mut RuntimeState, kind: ContextMenuKind, value: i32) {
    match kind {
        ContextMenuKind::Stroke => {
            state.stroke_width = value;
            if let Some(draft) = state.draft_annotation.as_mut() {
                match draft {
                    DraftAnnotation::Pen { width, .. }
                    | DraftAnnotation::Rectangle { width, .. }
                    | DraftAnnotation::Arrow { width, .. } => *width = value,
                    DraftAnnotation::Mosaic { .. } => {}
                }
                state.draft_dirty = true;
            }
        }
        ContextMenuKind::Font => {
            state.font_size = value;
            if let Some(text) = state.text_draft.as_mut() {
                text.font_size = value;
                reset_text_cursor_blink(state);
            }
        }
        ContextMenuKind::Mosaic => {
            state.mosaic_block_size = value;
            if let Some(DraftAnnotation::Mosaic { block_size, .. }) = state.draft_annotation.as_mut() {
                *block_size = value;
                state.draft_dirty = true;
            }
        }
        ContextMenuKind::Color => {
            let index = value as usize;
            if let Some((_, color)) = PALETTE.get(index) {
                set_active_annotation_color(state, index, *color);
            }
        }
        ContextMenuKind::CustomColor => return,
    }
    close_editor_context_menu(state);
}

fn begin_annotation_move(state: &mut RuntimeState, index: usize, anchor: (f64, f64), mode: AnnotationDragMode) -> bool {
    if index >= state.annotations.len() || state.moving.is_some() {
        return false;
    }
    let original = state.annotations.remove(index);
    state.moving = Some(MovingAnnotation { index, before: original.clone(), preview: original, anchor, mode });
    state.selected_annotation = None;
    state.static_dirty = true;
    state.draft_dirty = true;
    state.drag_anchor = Some(anchor);
    true
}

fn update_annotation_move(state: &mut RuntimeState, point: (f64, f64), selection: AnnotationRect) -> bool {
    let Some(moving) = state.moving.as_mut() else { return false };
    let moved = match moving.mode {
        AnnotationDragMode::Move => translate_annotation_in_selection(
            &moving.before,
            point.0 - moving.anchor.0,
            point.1 - moving.anchor.1,
            selection,
        ),
        AnnotationDragMode::ArrowStart | AnnotationDragMode::ArrowEnd => {
            let mut annotation = moving.before.clone();
            if let Annotation::Arrow { start, end, .. } = &mut annotation {
                let point = clamp_to_selection(point, selection);
                match moving.mode {
                    AnnotationDragMode::ArrowStart => *start = point,
                    AnnotationDragMode::ArrowEnd => *end = point,
                    AnnotationDragMode::Move => unreachable!(),
                }
            }
            annotation
        }
    };
    if moving.preview == moved {
        return false;
    }
    moving.preview = moved;
    state.last_text_click = None;
    state.draft_dirty = true;
    true
}

fn finish_annotation_move(state: &mut RuntimeState) -> bool {
    let Some(moving) = state.moving.take() else { return false };
    let index = moving.index.min(state.annotations.len());
    let after = moving.preview;
    if moving.before != after {
        state.undo_operations.push(EditOperation::Changed { index, before: moving.before, after: after.clone() });
        state.redo_operations.clear();
    }
    state.annotations.insert(index, after);
    state.selected_annotation = Some(index);
    state.static_dirty = true;
    state.draft_dirty = true;
    state.drag_anchor = None;
    true
}

fn cancel_annotation_move(state: &mut RuntimeState) -> bool {
    let Some(moving) = state.moving.take() else { return false };
    let index = moving.index.min(state.annotations.len());
    state.annotations.insert(index, moving.before);
    state.selected_annotation = Some(index);
    state.static_dirty = true;
    state.draft_dirty = true;
    state.drag_anchor = None;
    true
}

fn is_text_double_click(last: Option<(usize, Instant)>, index: usize, now: Instant) -> bool {
    last.is_some_and(|(previous_index, at)| {
        previous_index == index && now.saturating_duration_since(at) <= Duration::from_millis(420)
    })
}

fn hit_editor_annotation(state: &RuntimeState, point: (f64, f64)) -> Option<usize> {
    state
        .selected_annotation
        .filter(|index| state.annotations.get(*index).is_some_and(|ann| selected_annotation_hit_test(ann, point)))
        .or_else(|| state.annotations.iter().rposition(|ann| annotation_hit_test(ann, point)))
}

fn text_draft_contains_point(draft: &TextDraft, point: (f64, f64)) -> bool {
    let annotation = Annotation::Text {
        position: draft.position,
        text: format!("{}{}", draft.text, draft.preedit),
        font_size: draft.font_size,
        color: draft.color,
    };
    annotation_hit_test(&annotation, point)
}

fn selection_corners(selection: AnnotationRect) -> ((f64, f64), (f64, f64)) {
    (
        (f64::from(selection.x), f64::from(selection.y)),
        (f64::from(selection.x + selection.width), f64::from(selection.y + selection.height)),
    )
}

fn set_selection(state: &mut RuntimeState, selection: AnnotationRect, pixels: PixelRect) {
    state.selection_logical = Some(selection);
    state.selection_pixels = Some(pixels);
    state.selection_visuals_dirty = true;
    state.static_dirty |= !state.annotations.is_empty();
    state.draft_dirty = true;
    state.toolbar_dirty = true;
}

fn begin_selection_resize(state: &mut RuntimeState, point: (f64, f64)) -> bool {
    let (Some(before), Some(before_pixels)) = (state.selection_logical, state.selection_pixels) else {
        return false;
    };
    let Some(handle) = resize_hit_test(before, point) else {
        return false;
    };
    commit_text_draft(state);
    state.selected_annotation = None;
    state.last_text_click = None;
    state.draft_dirty = true;
    state.drag_anchor = None;
    state.resizing = Some(SelectionResize { handle, anchor: point, before, before_pixels });
    true
}

fn update_selection_resize(state: &mut RuntimeState, point: (f64, f64)) {
    let (Some(resize), Some(screen), Some(frame_size)) = (state.resizing, state.logical_size, state.frame_size) else {
        return;
    };
    let selection = resized_selection(resize.before, resize.handle, resize.anchor, point, screen);
    if state.selection_logical == Some(selection) {
        return;
    }
    let (start, end) = selection_corners(selection);
    if let Some(pixels) = pointer_selection(start, end, screen, frame_size) {
        set_selection(state, selection, pixels);
    }
}

fn finish_selection_resize(state: &mut RuntimeState) -> bool {
    let Some(resize) = state.resizing.take() else {
        return false;
    };
    if let (Some(selection), Some(pixels)) = (state.selection_logical, state.selection_pixels)
        && (selection, pixels) != (resize.before, resize.before_pixels)
    {
        state.undo_operations.push(EditOperation::SelectionResized {
            before: (resize.before, resize.before_pixels),
            after: (selection, pixels),
        });
        state.redo_operations.clear();
    }
    true
}

fn cancel_selection_resize(state: &mut RuntimeState) {
    if let Some(resize) = state.resizing.take() {
        set_selection(state, resize.before, resize.before_pixels);
    }
}

fn update_pointer_shape(state: &RuntimeState, point: (f64, f64)) {
    let (Some(device), Some(serial)) = (state.cursor_shape_device.as_ref(), state.pointer_enter_serial) else {
        return;
    };
    let handle = state.resizing.map(|resize| resize.handle).or_else(|| {
        if !state.editing || state.context_menu.is_some() || state.draft_annotation.is_some() || state.moving.is_some()
        {
            return None;
        }
        let selection = state.selection_logical?;
        if state.logical_size.is_some_and(|size| toolbar_layout(selection, size).bounds.contains(point)) {
            return None;
        }
        resize_hit_test(selection, point)
    });
    use wp_cursor_shape_device_v1::Shape;
    let shape = match handle {
        Some(ResizeHandle::TopLeft | ResizeHandle::BottomRight) => Shape::NwseResize,
        Some(ResizeHandle::TopRight | ResizeHandle::BottomLeft) => Shape::NeswResize,
        Some(ResizeHandle::Left | ResizeHandle::Right) => Shape::EwResize,
        Some(ResizeHandle::Top | ResizeHandle::Bottom) => Shape::NsResize,
        None => Shape::Default,
    };
    device.set_shape(serial, shape);
}

fn begin_editor_pointer(state: &mut RuntimeState, point: (f64, f64), connection: &Connection) {
    if let Some(menu) = state.context_menu.as_ref() {
        let kind = menu.kind;
        let choice = menu.hit_test(point);
        match (kind, choice) {
            (ContextMenuKind::CustomColor, _) => press_custom_color_editor(state, point),
            (ContextMenuKind::Color, Some(value)) if value == PALETTE.len() as i32 => {
                open_custom_color_editor(state);
            }
            (_, Some(value)) => apply_editor_context_choice(state, kind, value),
            (_, None) => close_editor_context_menu(state),
        }
        state.drag_anchor = None;
        redraw_editor(state, connection);
        return;
    }
    let Some(selection) = state.selection_logical else {
        return;
    };
    let toolbar = state.logical_size.map(|size| toolbar_layout(selection, size));
    if let Some(action) = toolbar.as_ref().and_then(|layout| layout.hit_test(point)) {
        state.drag_anchor = None;
        if action == ToolbarAction::Color {
            if let Some(screen) = state.logical_size {
                state.context_menu = Some(color_menu_layout(point, screen));
                state.menu_dirty = true;
            }
        } else {
            handle_toolbar_action(state, action);
        }
        redraw_editor(state, connection);
        return;
    }
    if begin_selection_resize(state, point) {
        redraw_editor(state, connection);
        return;
    }
    if !selection.contains(point) {
        state.drag_anchor = None;
        return;
    }
    let point = clamp_to_selection(point, selection);
    if state.text_draft.as_ref().is_some_and(|draft| text_draft_contains_point(draft, point)) {
        // Clicking the current text should not create an unrelated new annotation.
        redraw_editor(state, connection);
        return;
    }
    commit_text_draft(state);

    // Only the selected item's frame or control points are interactive.
    // Other objects can be selected by clicking their painted content.
    let clicked = hit_editor_annotation(state, point);
    if let Some(index) = clicked {
        let already_selected = state.selected_annotation == Some(index);
        let is_text = matches!(state.annotations.get(index), Some(Annotation::Text { .. }));
        let now = Instant::now();
        if is_text && already_selected && is_text_double_click(state.last_text_click, index, now) {
            if begin_existing_text_edit(state, index) {
                redraw_editor(state, connection);
            }
            return;
        }
        state.last_text_click = if is_text { Some((index, now)) } else { None };
        let mode = if already_selected {
            match state.annotations.get(index).and_then(|ann| arrow_handle_hit_test(ann, point)) {
                Some(ArrowHandle::Start) => AnnotationDragMode::ArrowStart,
                Some(ArrowHandle::End) => AnnotationDragMode::ArrowEnd,
                None => AnnotationDragMode::Move,
            }
        } else {
            AnnotationDragMode::Move
        };
        if begin_annotation_move(state, index, point, mode) {
            redraw_editor(state, connection);
        }
        return;
    }

    // An empty-space click only clears selection; the next gesture draws.
    if state.selected_annotation.take().is_some() {
        state.last_text_click = None;
        state.draft_dirty = true;
        redraw_editor(state, connection);
        return;
    }
    state.last_text_click = None;
    let color = active_annotation_color(state);
    match state.active_tool {
        EditorTool::Pen => {
            state.draft_annotation =
                Some(DraftAnnotation::Pen { points: vec![point], width: state.stroke_width, color });
            state.drag_anchor = Some(point);
        }
        EditorTool::Rectangle => {
            state.draft_annotation =
                Some(DraftAnnotation::Rectangle { start: point, end: point, width: state.stroke_width, color });
            state.drag_anchor = Some(point);
        }
        EditorTool::Arrow => {
            state.draft_annotation =
                Some(DraftAnnotation::Arrow { start: point, end: point, width: state.stroke_width, color });
            state.drag_anchor = Some(point);
        }
        EditorTool::Mosaic => {
            state.draft_annotation =
                Some(DraftAnnotation::Mosaic { start: point, end: point, block_size: state.mosaic_block_size });
            state.drag_anchor = Some(point);
        }
        EditorTool::Text => {
            let mut text = TextDraft::with_font_size(point, state.font_size);
            text.color = color;
            state.text_draft = Some(text);
            reset_text_cursor_blink(state);
            state.drag_anchor = None;
        }
    }
    state.draft_dirty = true;
    redraw_editor(state, connection);
}

fn update_editor_pointer(state: &mut RuntimeState, point: (f64, f64), connection: &Connection) {
    if state.resizing.is_some() {
        update_selection_resize(state, point);
        redraw_editor(state, connection);
        return;
    }
    let Some(selection) = state.selection_logical else {
        return;
    };
    let point = clamp_to_selection(point, selection);
    if state.moving.is_some() {
        if update_annotation_move(state, point, selection) {
            redraw_editor(state, connection);
        }
        return;
    }
    let Some(draft) = state.draft_annotation.as_mut() else {
        return;
    };
    let changed = match draft {
        DraftAnnotation::Pen { points, .. } => {
            let should_push = points.last().is_none_or(|last| {
                let dx = last.0 - point.0;
                let dy = last.1 - point.1;
                dx * dx + dy * dy >= 1.0
            });
            if should_push {
                points.push(point);
            }
            should_push
        }
        DraftAnnotation::Rectangle { end, .. }
        | DraftAnnotation::Arrow { end, .. }
        | DraftAnnotation::Mosaic { end, .. } => {
            if *end == point {
                false
            } else {
                *end = point;
                true
            }
        }
    };
    if !changed {
        return;
    }
    state.draft_dirty = true;
    redraw_editor(state, connection);
}

fn finish_editor_pointer(state: &mut RuntimeState, connection: &Connection) {
    state.drag_anchor = None;
    if finish_selection_resize(state) {
        redraw_editor(state, connection);
        return;
    }
    if finish_annotation_move(state) {
        redraw_editor(state, connection);
        return;
    }
    if let Some(draft) = state.draft_annotation.take() {
        let annotation = match draft {
            DraftAnnotation::Pen { points, width, color } if points.len() >= 2 => {
                Some(Annotation::Pen { points, width, color })
            }
            DraftAnnotation::Rectangle { start, end, width, color }
                if state.logical_size.is_some_and(|size| normalized_rect(start, end, size).is_some()) =>
            {
                Some(Annotation::Rectangle { start, end, width, color })
            }
            DraftAnnotation::Arrow { start, end, width, color } if start != end => {
                Some(Annotation::Arrow { start, end, width, color })
            }
            DraftAnnotation::Mosaic { start, end, block_size }
                if state.logical_size.is_some_and(|size| normalized_rect(start, end, size).is_some()) =>
            {
                Some(Annotation::Mosaic { start, end, block_size })
            }
            _ => None,
        };
        if let Some(annotation) = annotation {
            add_annotation(state, annotation);
        }
        state.draft_dirty = true;
        redraw_editor(state, connection);
    }
}

fn set_editor_tool(state: &mut RuntimeState, tool: EditorTool, connection: &Connection) {
    close_editor_context_menu(state);
    commit_text_draft(state);
    state.selected_annotation = None;
    state.last_text_click = None;
    state.draft_dirty = true;
    state.active_tool = tool;
    if state.draft_annotation.take().is_some() {
        state.draft_dirty = true;
    }
    state.toolbar_dirty = true;
    redraw_editor(state, connection);
}

fn handle_editor_key(state: &mut RuntimeState, key: u32, pressed: bool, connection: &Connection) {
    match key {
        LEFT_SHIFT_KEY | RIGHT_SHIFT_KEY => {
            state.shift_pressed = pressed;
            return;
        }
        LEFT_CTRL_KEY | RIGHT_CTRL_KEY => {
            state.ctrl_pressed = pressed;
            return;
        }
        _ if !pressed => return,
        _ => {}
    }

    if state.context_menu.as_ref().is_some_and(|menu| menu.kind == ContextMenuKind::CustomColor) {
        match key {
            ESC_KEY => back_to_color_palette(state),
            ENTER_KEY => {
                if !apply_custom_color_editor(state) {
                    state.menu_dirty = true;
                }
            }
            BACKSPACE_KEY => {
                if let Some(editor) = state.custom_color_editor.as_mut()
                    && editor.backspace_hex()
                {
                    state.menu_dirty = true;
                }
            }
            _ if !state.ctrl_pressed => {
                if let Some(ch) = key_to_ascii(key, state.shift_pressed)
                    && let Some(editor) = state.custom_color_editor.as_mut()
                    && editor.push_hex(ch)
                {
                    state.menu_dirty = true;
                }
            }
            _ => {}
        }
        redraw_editor(state, connection);
        return;
    }

    if state.resizing.is_some() {
        if key == ESC_KEY {
            cancel_selection_resize(state);
            redraw_editor(state, connection);
        }
        return;
    }

    if key == ESC_KEY {
        if state.context_menu.is_some() {
            close_editor_context_menu(state);
            redraw_editor(state, connection);
        } else if cancel_annotation_move(state) {
            redraw_editor(state, connection);
        } else if state.text_draft.is_some() {
            cancel_text_draft(state);
            redraw_editor(state, connection);
        } else if state.selected_annotation.take().is_some() {
            state.last_text_click = None;
            state.draft_dirty = true;
            redraw_editor(state, connection);
        } else {
            state.outcome = Some(RuntimeOutcome::Cancelled);
        }
        return;
    }

    if state.ctrl_pressed {
        match key {
            A_KEY => {
                if let Some(text) = state.text_draft.as_mut()
                    && text.select_all()
                {
                    reset_text_cursor_blink(state);
                    if state.text_input_enabled
                        && let Some(text_input) = state.text_input.clone()
                    {
                        send_text_input_state(state, &text_input);
                    }
                    redraw_editor(state, connection);
                }
            }
            Z_KEY => {
                let editing_text = state.text_draft.is_some();
                undo_editor(state);
                if editing_text
                    && state.text_input_enabled
                    && let Some(text_input) = state.text_input.clone()
                {
                    send_text_input_state(state, &text_input);
                }
                redraw_editor(state, connection);
            }
            Y_KEY => {
                let editing_text = state.text_draft.is_some();
                redo_editor(state);
                if editing_text
                    && state.text_input_enabled
                    && let Some(text_input) = state.text_input.clone()
                {
                    send_text_input_state(state, &text_input);
                }
                redraw_editor(state, connection);
            }
            _ => {}
        }
        return;
    }

    if state.text_draft.is_some() {
        let preedit_empty = state.text_draft.as_ref().is_some_and(|text| text.preedit.is_empty());
        if !state.text_input_enabled || preedit_empty {
            let mut changed = false;
            if let Some(text) = state.text_draft.as_mut() {
                changed = match key {
                    BACKSPACE_KEY => text.backspace(),
                    DELETE_KEY => text.delete_selected(),
                    ENTER_KEY => false,
                    _ => key_to_ascii(key, state.shift_pressed).is_some_and(|ch| text.insert_text(&ch.to_string())),
                };
            }
            if key == ENTER_KEY {
                commit_text_draft(state);
            } else if changed {
                reset_text_cursor_blink(state);
                if state.text_input_enabled
                    && let Some(text_input) = state.text_input.clone()
                {
                    send_text_input_state(state, &text_input);
                }
            }
        }
        redraw_editor(state, connection);
        return;
    }

    match key {
        25 => set_editor_tool(state, EditorTool::Pen, connection),
        19 => set_editor_tool(state, EditorTool::Rectangle, connection),
        30 => set_editor_tool(state, EditorTool::Arrow, connection),
        20 => set_editor_tool(state, EditorTool::Text, connection),
        ENTER_KEY => accept_editor(state, false),
        _ => {}
    }
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
            wl_registry::Event::Global { name, interface, version } if interface == CURSOR_SHAPE_MANAGER_V1 => {
                state.cursor_shape_manager = Some(registry.bind(name, version.min(1), queue_handle, ()));
            }
            wl_registry::Event::Global { name, interface, version } if interface == TEXT_INPUT_MANAGER_V3 => {
                state.text_input_manager = Some(registry.bind(name, version.min(1), queue_handle, ()));
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
                let pointer = seat.get_pointer(queue_handle, ());
                if let Some(manager) = state.cursor_shape_manager.as_ref() {
                    state.cursor_shape_device = Some(manager.get_pointer(&pointer, queue_handle, ()));
                }
                state.pointer = Some(pointer);
            }
            if capabilities.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(queue_handle, ()));
            }
        }
    }
}

impl Dispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for RuntimeState {
    fn event(
        _: &mut Self,
        _: &wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
        _: wp_cursor_shape_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, ()> for RuntimeState {
    fn event(
        _: &mut Self,
        _: &wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
        _: wp_cursor_shape_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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
        if let wl_pointer::Event::Enter { serial, .. } = &event {
            state.pointer_enter_serial = Some(*serial);
        }
        match event {
            wl_pointer::Event::Enter { surface_x, surface_y, .. }
            | wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                let current = (surface_x, surface_y);
                state.pointer_position = Some(current);
                update_pointer_shape(state, current);
                if state.editing {
                    if let Some(channel) = state.custom_color_editor.as_ref().and_then(|editor| editor.dragging_channel)
                    {
                        let changed = match channel {
                            0..=2 => move_custom_color_slider(state, channel, current.0),
                            CUSTOM_DRAG_SV => move_custom_color_square(state, current),
                            CUSTOM_DRAG_HUE => move_custom_color_hue(state, current.0),
                            _ => false,
                        };
                        if changed {
                            redraw_editor(state, connection);
                        }
                    } else if state.resizing.is_some() || state.draft_annotation.is_some() || state.moving.is_some() {
                        update_editor_pointer(state, current, connection);
                    }
                } else if let Some(anchor) = state.drag_anchor {
                    update_selection_visuals(state, Some(anchor), Some(current), connection);
                }
            }
            wl_pointer::Event::Button { button, state: WEnum::Value(button_state), .. }
                if button == BTN_RIGHT && button_state == wl_pointer::ButtonState::Pressed && state.editing =>
            {
                if let Some(current) = state.pointer_position {
                    begin_editor_context_menu(state, current, connection);
                }
            }
            wl_pointer::Event::Button { button, state: WEnum::Value(button_state), .. } if button == BTN_LEFT => {
                match button_state {
                    wl_pointer::ButtonState::Pressed => {
                        let Some(current) = state.pointer_position else {
                            return;
                        };
                        if state.editing {
                            begin_editor_pointer(state, current, connection);
                        } else {
                            state.drag_anchor = Some(current);
                            update_selection_visuals(state, Some(current), Some(current), connection);
                        }
                    }
                    wl_pointer::ButtonState::Released => {
                        if state.editing {
                            if let Some(editor) = state.custom_color_editor.as_mut()
                                && editor.dragging_channel.take().is_some()
                            {
                                return;
                            }
                            finish_editor_pointer(state, connection);
                            return;
                        }
                        if let (Some(anchor), Some(current), Some(logical_size), Some(frame_size)) =
                            (state.drag_anchor.take(), state.pointer_position, state.logical_size, state.frame_size)
                        {
                            let logical_selection = normalized_rect(anchor, current, logical_size);
                            let pixel_selection = pointer_selection(anchor, current, logical_size, frame_size);
                            if let (Some(logical_selection), Some(pixel_selection)) =
                                (logical_selection, pixel_selection)
                            {
                                if state.editor_enabled {
                                    state.editing = true;
                                    state.selection_logical = Some(logical_selection);
                                    state.selection_pixels = Some(pixel_selection);
                                    state.toolbar_dirty = true;
                                    update_selection_visuals(state, Some(anchor), Some(current), connection);
                                    redraw_editor(state, connection);
                                } else {
                                    update_selection_visuals(state, Some(anchor), Some(current), connection);
                                    state.outcome = Some(RuntimeOutcome::Selected(pixel_selection));
                                }
                            } else {
                                update_selection_visuals(state, None, None, connection);
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
        connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key { key, state: WEnum::Value(key_state), .. } = event {
            let pressed = matches!(key_state, wl_keyboard::KeyState::Pressed | wl_keyboard::KeyState::Repeated);
            if state.editor_enabled && state.editing {
                handle_editor_key(state, key, pressed, connection);
            } else if key == ESC_KEY && pressed {
                state.outcome = Some(RuntimeOutcome::Cancelled);
            }
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

impl Dispatch<wl_buffer::WlBuffer, CanvasBufferData> for RuntimeState {
    fn event(
        state: &mut Self,
        _proxy: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        data: &CanvasBufferData,
        connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release) {
            if data.layer == EditorLayerKind::Dimensions {
                if let Some(visuals) = state.visuals.as_mut() {
                    visuals.dimensions_layer.mark_released(data.slot);
                    if let Err(error) = visuals.redraw_dimensions() {
                        state.runtime_error = Some(error);
                    }
                    let _ = connection.flush();
                }
                return;
            }
            if let Some(surfaces) = state.editor_surfaces.as_mut() {
                surfaces.mark_released(data.layer, data.slot);
            }
            redraw_editor(state, connection);
        }
    }
}

impl Dispatch<zwp_text_input_manager_v3::ZwpTextInputManagerV3, ()> for RuntimeState {
    fn event(
        _state: &mut Self,
        _proxy: &zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        _event: zwp_text_input_manager_v3::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for RuntimeState {
    fn event(
        state: &mut Self,
        _proxy: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _data: &(),
        connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { .. } => {
                state.text_input_focus = true;
                update_text_input_activation(state, connection);
            }
            zwp_text_input_v3::Event::Leave { .. } => {
                state.text_input_focus = false;
                state.text_input_enabled = false;
                state.ime_pending_preedit = None;
                state.ime_pending_commit = None;
                state.ime_pending_delete = None;
                if let Some(text) = state.text_draft.as_mut()
                    && !text.preedit.is_empty()
                {
                    text.preedit.clear();
                    state.draft_dirty = true;
                    redraw_editor(state, connection);
                }
            }
            zwp_text_input_v3::Event::PreeditString { text, .. } => {
                state.ime_pending_preedit = Some(text.unwrap_or_default());
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                state.ime_pending_commit = Some(text.unwrap_or_default());
            }
            zwp_text_input_v3::Event::DeleteSurroundingText { before_length, after_length } => {
                state.ime_pending_delete = Some((before_length, after_length));
            }
            zwp_text_input_v3::Event::Done { serial } => {
                apply_text_input_done(state, serial, connection);
            }
            _ => {}
        }
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

    fn resize_test_state() -> RuntimeState {
        RuntimeState {
            editor_enabled: true,
            editing: true,
            logical_size: Some(PixelSize { width: 1000, height: 500 }),
            frame_size: Some(PixelSize { width: 1500, height: 750 }),
            selection_logical: Some(AnnotationRect { x: 100, y: 80, width: 400, height: 200 }),
            selection_pixels: Some(PixelRect { x: 150, y: 120, width: 600, height: 300 }),
            ..RuntimeState::default()
        }
    }

    #[test]
    fn selection_resize_updates_pixels_retains_annotations_and_supports_undo_redo() {
        let mut state = resize_test_state();
        let before = (state.selection_logical, state.selection_pixels);
        let annotation =
            Annotation::Arrow { start: (120.0, 100.0), end: (400.0, 200.0), width: 3, color: DEFAULT_COLOR };
        add_annotation(&mut state, annotation.clone());
        assert!(begin_selection_resize(&mut state, (500.0, 280.0)));
        update_selection_resize(&mut state, (600.0, 330.0));
        update_selection_resize(&mut state, (700.0, 380.0));
        assert_eq!(state.selection_logical, Some(AnnotationRect { x: 100, y: 80, width: 600, height: 300 }));
        assert_eq!(state.selection_pixels, Some(PixelRect { x: 150, y: 120, width: 900, height: 450 }));
        assert!(state.static_dirty && state.draft_dirty && state.toolbar_dirty && state.selection_visuals_dirty);
        assert_eq!(state.annotations, vec![annotation.clone()]);
        assert_eq!(state.selected_annotation, None);
        assert!(finish_selection_resize(&mut state));
        assert_eq!(state.undo_operations.len(), 2);
        let after = (state.selection_logical, state.selection_pixels);
        undo_editor(&mut state);
        assert_eq!((state.selection_logical, state.selection_pixels), before);
        assert_eq!(state.annotations, vec![annotation.clone()]);
        redo_editor(&mut state);
        assert_eq!((state.selection_logical, state.selection_pixels), after);
        undo_editor(&mut state);
        undo_editor(&mut state);
        assert!(state.annotations.is_empty());
        redo_editor(&mut state);
        redo_editor(&mut state);
        assert_eq!((state.selection_logical, state.selection_pixels), after);
        assert_eq!(state.annotations, vec![annotation]);
    }

    #[test]
    fn cancelled_or_unmoved_resize_preserves_exact_original_pixel_bounds_and_history() {
        let mut state = resize_test_state();
        // A fractional initial drag may cover fewer pixels than its rounded logical outline.
        state.selection_pixels = Some(PixelRect { x: 151, y: 121, width: 598, height: 298 });
        let before = (state.selection_logical, state.selection_pixels);
        assert!(!begin_selection_resize(&mut state, (300.0, 180.0)));
        assert!(begin_selection_resize(&mut state, (100.0, 80.0)));
        update_selection_resize(&mut state, (100.0, 80.0));
        assert!(finish_selection_resize(&mut state));
        assert_eq!((state.selection_logical, state.selection_pixels), before);
        assert!(state.undo_operations.is_empty());
        assert!(begin_selection_resize(&mut state, (100.0, 80.0)));
        update_selection_resize(&mut state, (10.0, 10.0));
        cancel_selection_resize(&mut state);
        assert_eq!((state.selection_logical, state.selection_pixels), before);
        assert!(state.resizing.is_none());
        assert!(state.undo_operations.is_empty());
    }

    #[test]
    fn resized_selection_is_used_by_save_and_copy() {
        for copy in [false, true] {
            let mut state = resize_test_state();
            assert!(begin_selection_resize(&mut state, (500.0, 180.0)));
            update_selection_resize(&mut state, (400.0, 180.0));
            finish_selection_resize(&mut state);
            accept_editor(&mut state, copy);
            let Some(RuntimeOutcome::Accepted { selection, copy: actual_copy, .. }) = state.outcome else {
                panic!("resized selection was not accepted");
            };
            assert_eq!(selection, PixelRect { x: 150, y: 120, width: 450, height: 300 });
            assert_eq!(actual_copy, copy);
        }
    }

    #[test]
    fn beginning_resize_commits_pending_text_before_selection_history() {
        let mut state = resize_test_state();
        let mut text = TextDraft::new((200.0, 150.0));
        text.insert_text("保留标注");
        state.text_draft = Some(text);
        assert!(begin_selection_resize(&mut state, (500.0, 180.0)));
        assert!(state.text_draft.is_none());
        assert!(matches!(state.annotations.as_slice(), [Annotation::Text { text, .. }] if text == "保留标注"));
        update_selection_resize(&mut state, (600.0, 180.0));
        finish_selection_resize(&mut state);
        assert_eq!(state.undo_operations.len(), 2);
    }

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
    fn dimension_label_uses_export_pixels_at_fractional_and_axis_scales() {
        let logical_size = PixelSize { width: 1000, height: 500 };
        for (frame_size, expected) in [
            (PixelSize { width: 1000, height: 500 }, "401 × 201 px"),
            (PixelSize { width: 1250, height: 625 }, "501 × 252 px"),
            (PixelSize { width: 2000, height: 1000 }, "801 × 401 px"),
            (PixelSize { width: 2000, height: 1500 }, "801 × 602 px"),
        ] {
            let start = (100.25, 50.25);
            let end = (500.5, 250.5);
            let dimensions = selection_dimensions(start, end, logical_size, frame_size).unwrap();
            assert_eq!(dimensions.text, expected);
            assert_eq!(selection_dimensions(end, start, logical_size, frame_size), Some(dimensions));
        }
    }

    #[test]
    fn dimension_label_clips_to_output_and_hides_for_empty_selection() {
        let logical_size = PixelSize { width: 1000, height: 500 };
        let frame_size = PixelSize { width: 2000, height: 1000 };
        let dimensions = selection_dimensions((-100.0, -50.0), (1100.0, 550.0), logical_size, frame_size).unwrap();
        assert_eq!(dimensions.text, "2000 × 1000 px");
        for (start, end) in [
            ((100.0, 50.0), (100.0, 50.0)),
            ((100.25, 50.25), (100.25, 50.25)),
            ((100.0, 50.0), (100.0, 250.0)),
            ((1100.0, 550.0), (1200.0, 600.0)),
        ] {
            assert_eq!(selection_dimensions(start, end, logical_size, frame_size), None);
        }
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

    #[test]
    fn utf8_suffix_delete_never_splits_chinese_codepoint() {
        let mut text = "abc中文".to_string();
        delete_utf8_suffix_bytes(&mut text, 3);
        assert_eq!(text, "abc中");
        delete_utf8_suffix_bytes(&mut text, 1);
        assert_eq!(text, "abc");
    }

    #[test]
    fn undo_during_text_entry_preserves_other_annotations() {
        let mut state = RuntimeState::default();
        state.annotations.push(Annotation::Rectangle {
            start: (1.0, 1.0),
            end: (10.0, 10.0),
            width: 3,
            color: DEFAULT_COLOR,
        });
        let mut text = TextDraft::new((2.0, 3.0));
        assert!(text.insert_text("hello"));
        assert!(text.insert_text("世界"));
        state.text_draft = Some(text);
        undo_editor(&mut state);
        assert_eq!(state.text_draft.as_ref().unwrap().text, "hello");
        assert_eq!(state.annotations.len(), 1);
        redo_editor(&mut state);
        assert_eq!(state.text_draft.as_ref().unwrap().text, "hello世界");
        assert_eq!(state.annotations.len(), 1);
        state.text_draft = None;
        undo_editor(&mut state);
        assert!(state.annotations.is_empty());
    }

    #[test]
    fn ime_commit_replaces_selected_chinese_text_and_is_undoable() {
        let mut text = TextDraft::new((2.0, 2.0));
        assert!(text.insert_text("旧内容"));
        assert!(text.select_all());
        assert!(apply_ime_text_change(&mut text, Some((9, 0)), "新内容"));
        assert_eq!(text.text, "新内容");
        assert!(!text.selected_all);
        assert!(text.undo());
        assert_eq!(text.text, "旧内容");
        assert!(text.redo());
        assert_eq!(text.text, "新内容");
        assert!(!apply_ime_text_change(&mut text, None, ""));
    }

    #[test]
    fn context_choice_updates_current_draft_without_rewriting_older_annotations() {
        let mut state = RuntimeState { stroke_width: 3, font_size: 24, ..RuntimeState::default() };
        state.annotations.push(Annotation::Rectangle {
            start: (10.0, 10.0),
            end: (50.0, 50.0),
            width: 3,
            color: DEFAULT_COLOR,
        });
        state.draft_annotation =
            Some(DraftAnnotation::Rectangle { start: (50.0, 50.0), end: (80.0, 80.0), width: 3, color: DEFAULT_COLOR });
        state.context_menu =
            Some(context_menu_layout(EditorTool::Rectangle, (40.0, 40.0), PixelSize { width: 800, height: 600 }));
        apply_editor_context_choice(&mut state, ContextMenuKind::Stroke, 8);
        assert_eq!(state.stroke_width, 8);
        assert!(state.draft_dirty && state.menu_dirty);
        assert!(state.context_menu.is_none());
        assert!(matches!(state.draft_annotation.as_ref(), Some(DraftAnnotation::Rectangle { width: 8, .. })));
        assert!(matches!(state.annotations[0], Annotation::Rectangle { width: 3, .. }));
        let mut text = TextDraft::with_font_size((50.0, 50.0), 24);
        text.insert_text("中文");
        state.text_draft = Some(text);
        apply_editor_context_choice(&mut state, ContextMenuKind::Font, 40);
        assert_eq!(state.font_size, 40);
        assert_eq!(state.text_draft.as_ref().unwrap().font_size, 40);
        assert_eq!(state.text_draft.as_ref().unwrap().text, "中文");
    }

    #[test]
    fn moving_existing_annotations_preserves_order_and_supports_undo_redo() {
        let color = PALETTE[6].1;
        let variants = [
            Annotation::Pen { points: vec![(30.0, 40.0), (80.0, 60.0)], width: 3, color },
            Annotation::Rectangle { start: (30.0, 40.0), end: (80.0, 90.0), width: 3, color },
            Annotation::Arrow { start: (30.0, 40.0), end: (80.0, 60.0), width: 3, color },
            Annotation::Text { position: (30.0, 40.0), text: "文字".into(), font_size: 24, color },
            Annotation::Mosaic { start: (30.0, 40.0), end: (80.0, 90.0), block_size: 18 },
        ];
        let clip = AnnotationRect { x: 0, y: 0, width: 800, height: 600 };
        for initial in variants {
            let mut state = RuntimeState::default();
            add_annotation(
                &mut state,
                Annotation::Text { position: (400.0, 400.0), text: "higher".into(), font_size: 24, color },
            );
            let higher = state.annotations[0].clone();
            add_annotation(&mut state, initial.clone());
            assert_eq!(state.selected_annotation, Some(1));
            // Only one selection is active, but an older annotation can be reselected.
            assert_eq!(hit_editor_annotation(&state, (412.0, 408.0)), Some(0));
            assert!(begin_annotation_move(&mut state, 1, (40.0, 50.0), AnnotationDragMode::Move));
            assert_eq!(state.selected_annotation, None);
            assert!(state.moving.is_some());
            assert_eq!(state.annotations.len(), 1);
            assert!(update_annotation_move(&mut state, (70.0, 75.0), clip));
            assert!(finish_annotation_move(&mut state));
            assert_eq!(state.annotations.len(), 2);
            assert_eq!(state.annotations[0], higher);
            assert_ne!(state.annotations[1], initial);
            let moved = state.annotations[1].clone();
            assert!(begin_annotation_move(&mut state, 1, (70.0, 75.0), AnnotationDragMode::Move));
            assert!(cancel_annotation_move(&mut state));
            undo_editor(&mut state);
            assert_eq!(state.annotations[1], initial);
            assert_eq!(state.selected_annotation, None);
            redo_editor(&mut state);
            assert_eq!(state.annotations[1], moved);
            assert_eq!(state.selected_annotation, None);
        }
    }

    #[test]
    fn escape_cancels_move_without_changing_history() {
        let mut state = RuntimeState::default();
        let initial = Annotation::Arrow { start: (20.0, 20.0), end: (100.0, 60.0), width: 5, color: PALETTE[1].1 };
        add_annotation(&mut state, initial.clone());
        let history_len = state.undo_operations.len();
        assert!(begin_annotation_move(&mut state, 0, (40.0, 30.0), AnnotationDragMode::Move));
        assert!(update_annotation_move(
            &mut state,
            (100.0, 90.0),
            AnnotationRect { x: 0, y: 0, width: 800, height: 600 },
        ));
        assert!(cancel_annotation_move(&mut state));
        assert_eq!(state.annotations, vec![initial]);
        assert_eq!(state.undo_operations.len(), history_len);
        assert_eq!(state.selected_annotation, Some(0));
        assert!(begin_annotation_move(&mut state, 0, (40.0, 30.0), AnnotationDragMode::Move));
        assert!(cancel_annotation_move(&mut state));
    }

    #[test]
    fn only_one_annotation_is_selected_and_clicking_old_items_can_reselect() {
        let color = PALETTE[4].1;
        let mut state = RuntimeState::default();
        let old = Annotation::Rectangle { start: (20.0, 20.0), end: (90.0, 90.0), width: 3, color };
        let newest = Annotation::Arrow { start: (140.0, 100.0), end: (200.0, 150.0), width: 5, color };
        add_annotation(&mut state, old.clone());
        assert_eq!(state.selected_annotation, Some(0));
        add_annotation(&mut state, newest.clone());
        assert_eq!(state.selected_annotation, Some(1));
        assert_eq!(hit_editor_annotation(&state, (25.0, 25.0)), Some(0));
        assert_eq!(hit_editor_annotation(&state, (165.0, 120.0)), Some(1));
        assert_eq!(hit_editor_annotation(&state, (300.0, 300.0)), None);
        assert!(begin_annotation_move(&mut state, 0, (25.0, 25.0), AnnotationDragMode::Move));
        assert_eq!(state.selected_annotation, None);
        assert!(finish_annotation_move(&mut state));
        assert_eq!(state.selected_annotation, Some(0));
        assert_eq!(state.annotations, vec![old.clone(), newest.clone()]);
        assert!(begin_annotation_move(&mut state, 1, (165.0, 120.0), AnnotationDragMode::Move));
        assert!(finish_annotation_move(&mut state));
        assert_eq!(state.selected_annotation, Some(1));

        let next = Annotation::Text { position: (240.0, 220.0), text: "新标注".into(), font_size: 24, color };
        add_annotation(&mut state, next);
        assert_eq!(state.selected_annotation, Some(2));
        undo_editor(&mut state);
        assert_eq!(state.selected_annotation, None);
        redo_editor(&mut state);
        assert_eq!(state.selected_annotation, None);
        assert_eq!(hit_editor_annotation(&state, (165.0, 120.0)), Some(1));
    }

    #[test]
    fn arrow_control_points_change_direction_and_support_undo_redo() {
        let mut state = RuntimeState::default();
        let arrow = Annotation::Arrow { start: (60.0, 90.0), end: (210.0, 130.0), width: 4, color: PALETTE[6].1 };
        let clip = AnnotationRect { x: 0, y: 0, width: 600, height: 500 };
        add_annotation(&mut state, arrow.clone());
        assert!(begin_annotation_move(&mut state, 0, (210.0, 130.0), AnnotationDragMode::ArrowEnd));
        assert!(update_annotation_move(&mut state, (20.0, 40.0), clip));
        assert!(matches!(
            state.moving.as_ref().map(|drag| &drag.preview),
            Some(Annotation::Arrow { start: (60.0, 90.0), end: (20.0, 40.0), .. })
        ));
        assert!(finish_annotation_move(&mut state));
        let reversed = state.annotations[0].clone();
        assert_ne!(arrow, reversed);
        assert_eq!(state.selected_annotation, Some(0));
        undo_editor(&mut state);
        assert_eq!(state.annotations[0], arrow);
        redo_editor(&mut state);
        assert_eq!(state.annotations[0], reversed);
        assert!(begin_annotation_move(&mut state, 0, (60.0, 90.0), AnnotationDragMode::ArrowStart));
        assert!(update_annotation_move(&mut state, (290.0, 260.0), clip));
        assert!(finish_annotation_move(&mut state));
        assert!(matches!(state.annotations[0], Annotation::Arrow { start: (290.0, 260.0), end: (20.0, 40.0), .. }));
        undo_editor(&mut state);
        assert_eq!(state.annotations[0], reversed);
    }

    #[test]
    fn double_click_existing_text_reopens_same_annotation_and_tracks_content_edits() {
        let mut state = RuntimeState::default();
        let color = PALETTE[8].1;
        add_annotation(&mut state, Annotation::Rectangle { start: (12.0, 12.0), end: (30.0, 30.0), width: 3, color });
        let old = Annotation::Text { position: (75.0, 120.0), text: "原来文字".into(), font_size: 40, color };
        add_annotation(&mut state, old.clone());
        assert_eq!(state.selected_annotation, Some(1));
        let now = Instant::now();
        assert!(is_text_double_click(Some((1, now - Duration::from_millis(200))), 1, now));
        assert!(!is_text_double_click(Some((1, now - Duration::from_millis(600))), 1, now));
        assert!(!is_text_double_click(Some((0, now - Duration::from_millis(200))), 1, now));
        assert!(begin_existing_text_edit(&mut state, 1));
        assert_eq!(state.annotations.len(), 1);
        assert_eq!(state.selected_annotation, None);
        assert_eq!(state.text_draft.as_ref().unwrap().text, "原来文字");
        assert_eq!(state.text_draft.as_ref().unwrap().font_size, 40);
        assert_eq!(state.text_draft.as_ref().unwrap().color, color);
        let draft = state.text_draft.as_mut().unwrap();
        assert!(draft.select_all());
        assert!(draft.insert_text("重新编辑中文"));
        commit_text_draft(&mut state);
        assert_eq!(state.annotations.len(), 2);
        assert_eq!(state.selected_annotation, Some(1));
        assert!(
            matches!(&state.annotations[1], Annotation::Text { text, font_size: 40, .. } if text == "重新编辑中文")
        );
        let updated = state.annotations[1].clone();
        undo_editor(&mut state);
        assert_eq!(state.annotations[1], old);
        redo_editor(&mut state);
        assert_eq!(state.annotations[1], updated);
        assert!(begin_existing_text_edit(&mut state, 1));
        assert_eq!(state.text_draft.as_ref().unwrap().text, "重新编辑中文");
        cancel_text_draft(&mut state);
        assert_eq!(state.annotations[1], updated);
        assert_eq!(state.selected_annotation, Some(1));
        assert!(state.editing_existing_text.is_none());
    }

    #[test]
    fn deleting_existing_text_can_be_undone_and_reedit_without_changes_preserves_history() {
        let mut state = RuntimeState::default();
        let original =
            Annotation::Text { position: (25.0, 30.0), text: "删除我".into(), font_size: 24, color: DEFAULT_COLOR };
        add_annotation(&mut state, original.clone());
        let history_before = state.undo_operations.len();
        assert!(begin_existing_text_edit(&mut state, 0));
        commit_text_draft(&mut state);
        assert_eq!(state.undo_operations.len(), history_before);
        assert_eq!(state.annotations, vec![original.clone()]);
        assert!(begin_existing_text_edit(&mut state, 0));
        let draft = state.text_draft.as_mut().unwrap();
        assert!(draft.select_all());
        assert!(draft.delete_selected());
        commit_text_draft(&mut state);
        assert!(state.annotations.is_empty());
        assert_eq!(state.selected_annotation, None);
        undo_editor(&mut state);
        assert_eq!(state.annotations, vec![original.clone()]);
        redo_editor(&mut state);
        assert!(state.annotations.is_empty());
    }

    #[test]
    fn color_and_mosaic_context_preferences_apply_to_new_drawings() {
        let mut state =
            RuntimeState { stroke_width: 3, font_size: 24, mosaic_block_size: 18, ..RuntimeState::default() };
        apply_editor_context_choice(&mut state, ContextMenuKind::Color, 7);
        assert_eq!(state.color_index, 7);
        assert!(state.toolbar_dirty);
        apply_editor_context_choice(&mut state, ContextMenuKind::Mosaic, 32);
        assert_eq!(state.mosaic_block_size, 32);
        let size = PixelSize { width: 800, height: 600 };
        let menu = context_menu_layout(EditorTool::Mosaic, (300.0, 200.0), size);
        assert_eq!(menu.kind, ContextMenuKind::Mosaic);
        assert_eq!(menu.options.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [8, 12, 18, 24, 32, 48]);
    }

    #[test]
    fn custom_color_selection_updates_only_active_and_new_annotations() {
        let screen = PixelSize { width: 1024, height: 768 };
        let mut state =
            RuntimeState { logical_size: Some(screen), custom_color: DEFAULT_COLOR, ..RuntimeState::default() };
        let old = Annotation::Rectangle { start: (15.0, 15.0), end: (60.0, 60.0), width: 3, color: PALETTE[5].1 };
        state.annotations.push(old.clone());
        state.draft_annotation =
            Some(DraftAnnotation::Arrow { start: (70.0, 70.0), end: (110.0, 110.0), width: 3, color: DEFAULT_COLOR });
        let mut text = TextDraft::new((50.0, 50.0));
        text.insert_text("已经输入");
        state.text_draft = Some(text);
        state.context_menu = Some(color_menu_layout((240.0, 220.0), screen));
        open_custom_color_editor(&mut state);
        assert_eq!(state.context_menu.as_ref().unwrap().kind, ContextMenuKind::CustomColor);
        let hex = state.context_menu.as_ref().unwrap().options[3].1;
        press_custom_color_editor(&mut state, (f64::from(hex.x + 7), f64::from(hex.y + 7)));
        let editor = state.custom_color_editor.as_mut().unwrap();
        assert!(editor.editing_hex);
        for ch in "2d8bc7".chars() {
            editor.push_hex(ch);
        }
        assert_eq!(editor.chosen_color(), Some([45, 139, 199, 255]));
        let confirm = state.context_menu.as_ref().unwrap().options[4].1;
        press_custom_color_editor(&mut state, (f64::from(confirm.x + 9), f64::from(confirm.y + 9)));
        assert_eq!(state.color_index, PALETTE.len());
        assert_eq!(active_annotation_color(&state), [45, 139, 199, 255]);
        assert_eq!(state.custom_color, [45, 139, 199, 255]);
        assert!(state.context_menu.is_none());
        assert_eq!(state.annotations[0], old);
        assert_eq!(state.text_draft.as_ref().unwrap().color, [45, 139, 199, 255]);
        assert!(matches!(state.draft_annotation, Some(DraftAnnotation::Arrow { color: [45, 139, 199, 255], .. })));
        state.context_menu = Some(color_menu_layout((240.0, 220.0), screen));
        open_custom_color_editor(&mut state);
        assert_eq!(state.custom_color_editor.as_ref().unwrap().color, [45, 139, 199, 255]);
        back_to_color_palette(&mut state);
        assert_eq!(state.context_menu.as_ref().unwrap().kind, ContextMenuKind::Color);
        assert!(state.custom_color_editor.is_none());
        apply_editor_context_choice(&mut state, ContextMenuKind::Color, 2);
        assert_eq!(active_annotation_color(&state), PALETTE[2].1);
        assert_eq!(state.custom_color, [45, 139, 199, 255]);
    }

    #[test]
    fn custom_hex_requires_complete_input_and_slider_drag_changes_preview() {
        let screen = PixelSize { width: 1024, height: 768 };
        let mut state =
            RuntimeState { logical_size: Some(screen), custom_color: DEFAULT_COLOR, ..RuntimeState::default() };
        state.context_menu = Some(color_menu_layout((200.0, 170.0), screen));
        open_custom_color_editor(&mut state);
        let hex = state.context_menu.as_ref().unwrap().options[3].1;
        press_custom_color_editor(&mut state, (f64::from(hex.x + 10), f64::from(hex.y + 10)));
        for ch in "11".chars() {
            state.custom_color_editor.as_mut().unwrap().push_hex(ch);
        }
        assert!(!apply_custom_color_editor(&mut state));
        assert!(state.context_menu.is_some());
        let red_slider = state.context_menu.as_ref().unwrap().options[0].1;
        press_custom_color_editor(&mut state, (f64::from(red_slider.x + 68), f64::from(red_slider.y + 10)));
        assert_eq!(state.custom_color_editor.as_ref().unwrap().dragging_channel, Some(0));
        assert_eq!(state.custom_color_editor.as_ref().unwrap().color[0], 0);
        assert!(move_custom_color_slider(&mut state, 0, f64::from(red_slider.x + red_slider.width - 10)));
        assert_eq!(state.custom_color_editor.as_ref().unwrap().color[0], 255);
        assert!(!state.custom_color_editor.as_ref().unwrap().editing_hex);
        assert!(apply_custom_color_editor(&mut state));
        assert_eq!(state.custom_color[0], 255);
    }

    #[test]
    fn custom_color_rectangle_and_hue_support_press_drag_and_apply() {
        let screen = PixelSize { width: 1024, height: 768 };
        let mut state =
            RuntimeState { logical_size: Some(screen), custom_color: [255, 0, 0, 255], ..RuntimeState::default() };
        state.context_menu = Some(color_menu_layout((400.0, 320.0), screen));
        open_custom_color_editor(&mut state);
        let menu = state.context_menu.as_ref().unwrap();
        let square = menu.options.iter().find(|(id, _)| *id == CUSTOM_SV_ID).unwrap().1;
        let hue_strip = menu.options.iter().find(|(id, _)| *id == CUSTOM_HUE_ID).unwrap().1;
        assert_eq!(menu.bounds.width, CONTEXT_MENU_SIZE.width as i32);
        assert_eq!(menu.bounds.height, CONTEXT_MENU_SIZE.height as i32);

        // Drag hue to green, then choose the fully saturated, bright corner.
        let green_hue_x = f64::from(hue_strip.x) + f64::from(hue_strip.width - 1) / 3.0;
        press_custom_color_editor(&mut state, (green_hue_x, f64::from(hue_strip.y + 6)));
        assert_eq!(state.custom_color_editor.as_ref().unwrap().dragging_channel, Some(CUSTOM_DRAG_HUE));
        // Hue changes preserve saturation; the 216-pixel hue strip chooses the
        // nearest green pixel, so the result need not be exactly 120 degrees.
        let light_green = state.custom_color_editor.as_ref().unwrap().color;
        assert!((i32::from(light_green[0]) - 76).abs() <= 3);
        assert_eq!(light_green[1], 255);
        assert!((i32::from(light_green[2]) - 76).abs() <= 3);
        state.custom_color_editor.as_mut().unwrap().dragging_channel.take();

        let saturated = (f64::from(square.x + square.width - 1), f64::from(square.y));
        press_custom_color_editor(&mut state, saturated);
        assert_eq!(state.custom_color_editor.as_ref().unwrap().dragging_channel, Some(CUSTOM_DRAG_SV));
        let green = state.custom_color_editor.as_ref().unwrap().color;
        assert!(green[0] <= 3 && green[1] == 255 && green[2] <= 3);
        assert!(move_custom_color_square(&mut state, (f64::from(square.x), f64::from(square.y))));
        assert_eq!(state.custom_color_editor.as_ref().unwrap().color, [255, 255, 255, 255]);
        assert!(move_custom_color_square(
            &mut state,
            (f64::from(square.x + square.width - 1), f64::from(square.y + square.height - 1))
        ));
        assert_eq!(state.custom_color_editor.as_ref().unwrap().color, [0, 0, 0, 255]);
        assert!(move_custom_color_square(&mut state, saturated));
        let color_editor = state.custom_color_editor.as_ref().unwrap();
        assert_eq!(color_editor.color, green);
        assert_eq!(color_editor.hex_input, format!("{:02X}{:02X}{:02X}", green[0], green[1], green[2]));
        assert!(state.menu_dirty);
        state.custom_color_editor.as_mut().unwrap().dragging_channel.take();

        assert!(apply_custom_color_editor(&mut state));
        assert_eq!(state.custom_color, green);
        assert_eq!(active_annotation_color(&state), green);
        assert_eq!(state.color_index, PALETTE.len());
    }

    #[test]
    fn custom_color_drag_outside_bounds_clamps_and_keeps_rgb_and_hex_in_sync() {
        let screen = PixelSize { width: 1024, height: 768 };
        let mut state =
            RuntimeState { logical_size: Some(screen), custom_color: [255, 0, 0, 255], ..RuntimeState::default() };
        state.context_menu = Some(color_menu_layout((120.0, 120.0), screen));
        open_custom_color_editor(&mut state);
        let square = state.context_menu.as_ref().unwrap().options.iter().find(|(id, _)| *id == CUSTOM_SV_ID).unwrap().1;
        assert!(move_custom_color_square(&mut state, (f64::from(square.x - 500), f64::from(square.y - 500))));
        assert_eq!(state.custom_color_editor.as_ref().unwrap().color, [255, 255, 255, 255]);
        assert!(move_custom_color_hue(&mut state, 10000.0));
        assert!((state.custom_color_editor.as_ref().unwrap().hue - 360.0).abs() < 0.001);
        assert!(move_custom_color_square(&mut state, (f64::from(square.x + 10000), f64::from(square.y + 10000))));
        assert_eq!(state.custom_color_editor.as_ref().unwrap().color, [0, 0, 0, 255]);
        assert_eq!(state.custom_color_editor.as_ref().unwrap().hex_input, "000000");
        let editor = state.custom_color_editor.as_mut().unwrap();
        assert!(editor.set_channel(1, 128));
        assert_eq!(editor.color, [0, 128, 0, 255]);
        assert!(editor.hue > 110.0 && editor.hue < 130.0);
        assert_eq!(editor.hex_input, "008000");
    }

    #[test]
    fn overlay_requires_pointer_and_keyboard_capabilities() {
        assert!(require_input_capabilities(true, true).is_ok());
        assert!(matches!(require_input_capabilities(false, true), Err(CaptureError::Unsupported(_))));
        assert!(matches!(require_input_capabilities(true, false), Err(CaptureError::Unsupported(_))));
        assert!(matches!(require_input_capabilities(false, false), Err(CaptureError::Unsupported(_))));
    }
}
