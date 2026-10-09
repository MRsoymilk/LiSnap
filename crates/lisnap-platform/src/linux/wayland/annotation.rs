use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use fontdue::{Font, FontSettings};
use lisnap_core::{CaptureError, CaptureFrame, PixelRect, PixelSize, RgbaImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EditorTool {
    #[default]
    Pen,
    Rectangle,
    Arrow,
    Text,
    Mosaic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolbarAction {
    Color,
    Tool(EditorTool),
    Undo,
    Redo,
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RectI {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

impl RectI {
    pub(crate) fn contains(self, point: (f64, f64)) -> bool {
        point.0 >= f64::from(self.x)
            && point.1 >= f64::from(self.y)
            && point.0 < f64::from(self.x + self.width)
            && point.1 < f64::from(self.y + self.height)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Annotation {
    Pen { points: Vec<(f64, f64)>, width: i32, color: [u8; 4] },
    Rectangle { start: (f64, f64), end: (f64, f64), width: i32, color: [u8; 4] },
    Arrow { start: (f64, f64), end: (f64, f64), width: i32, color: [u8; 4] },
    Text { position: (f64, f64), text: String, font_size: i32, color: [u8; 4] },
    Mosaic { start: (f64, f64), end: (f64, f64), block_size: i32 },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DraftAnnotation {
    Pen { points: Vec<(f64, f64)>, width: i32, color: [u8; 4] },
    Rectangle { start: (f64, f64), end: (f64, f64), width: i32, color: [u8; 4] },
    Arrow { start: (f64, f64), end: (f64, f64), width: i32, color: [u8; 4] },
    Mosaic { start: (f64, f64), end: (f64, f64), block_size: i32 },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextDraft {
    pub(crate) position: (f64, f64),
    pub(crate) text: String,
    pub(crate) preedit: String,
    pub(crate) font_size: i32,
    pub(crate) color: [u8; 4],
    pub(crate) selected_all: bool,
    undo_history: Vec<String>,
    redo_history: Vec<String>,
}

impl TextDraft {
    pub(crate) fn new(position: (f64, f64)) -> Self {
        Self {
            position,
            text: String::new(),
            preedit: String::new(),
            font_size: 24,
            color: DEFAULT_COLOR,
            selected_all: false,
            undo_history: Vec::new(),
            redo_history: Vec::new(),
        }
    }

    pub(crate) fn with_font_size(position: (f64, f64), font_size: i32) -> Self {
        let mut text = Self::new(position);
        text.font_size = font_size;
        text
    }

    pub(crate) fn select_all(&mut self) -> bool {
        if self.text.is_empty() || !self.preedit.is_empty() || self.selected_all {
            return false;
        }
        self.selected_all = true;
        true
    }

    pub(crate) fn update_text(&mut self, updated: String) -> bool {
        let previous = std::mem::replace(&mut self.text, updated);
        let changed = previous != self.text;
        self.selected_all = false;
        if changed {
            if self.undo_history.len() >= 128 {
                self.undo_history.remove(0);
            }
            self.undo_history.push(previous);
            self.redo_history.clear();
        }
        changed
    }

    pub(crate) fn insert_text(&mut self, input: &str) -> bool {
        if input.is_empty() {
            return false;
        }
        let updated = if self.selected_all { input.to_owned() } else { format!("{}{}", self.text, input) };
        self.update_text(updated)
    }

    pub(crate) fn backspace(&mut self) -> bool {
        if self.selected_all {
            return self.update_text(String::new());
        }
        let mut updated = self.text.clone();
        if updated.pop().is_none() {
            return false;
        }
        self.update_text(updated)
    }

    pub(crate) fn delete_selected(&mut self) -> bool {
        self.selected_all && self.update_text(String::new())
    }

    pub(crate) fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_history.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.text, previous);
        self.redo_history.push(current);
        self.selected_all = false;
        self.preedit.clear();
        true
    }

    pub(crate) fn redo(&mut self) -> bool {
        let Some(next) = self.redo_history.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.text, next);
        self.undo_history.push(current);
        self.selected_all = false;
        self.preedit.clear();
        true
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ToolbarLayout {
    pub(crate) bounds: RectI,
    pub(crate) buttons: Vec<(ToolbarAction, RectI)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextMenuKind {
    Stroke,
    Font,
    Color,
    CustomColor,
    Mosaic,
}

#[derive(Debug, Clone)]
pub(crate) struct ContextMenu {
    pub(crate) kind: ContextMenuKind,
    pub(crate) bounds: RectI,
    pub(crate) options: Vec<(i32, RectI)>,
}

impl ContextMenu {
    pub(crate) fn hit_test(&self, point: (f64, f64)) -> Option<i32> {
        self.options.iter().find_map(|(value, rect)| rect.contains(point).then_some(*value))
    }
}

// Menu backing stores the largest popup. Ordinary popups keep their compact dimensions.
pub(crate) const CONTEXT_MENU_SIZE: PixelSize = PixelSize { width: 244, height: 440 };
const SV_WIDTH: usize = 216;
const SV_HEIGHT: usize = 128;
const HUE_STRIP_WIDTH: usize = 216;
const HUE_STRIP_HEIGHT: usize = 16;
const STANDARD_MENU_WIDTH: i32 = 190;
const STANDARD_MENU_HEIGHT: i32 = 296;
pub(crate) const CUSTOM_SV_ID: i32 = 6;
pub(crate) const CUSTOM_HUE_ID: i32 = 7;
pub(crate) const CUSTOM_DRAG_SV: usize = 3;
pub(crate) const CUSTOM_DRAG_HUE: usize = 4;
pub(crate) const DEFAULT_COLOR: [u8; 4] = [255, 76, 76, 255];
pub(crate) const PALETTE: [(&str, [u8; 4]); 12] = [
    ("RED", [255, 76, 76, 255]),
    ("ORANGE", [255, 151, 61, 255]),
    ("YELLOW", [255, 218, 78, 255]),
    ("LIME", [164, 221, 70, 255]),
    ("GREEN", [55, 197, 119, 255]),
    ("CYAN", [58, 204, 216, 255]),
    ("BLUE", [70, 142, 247, 255]),
    ("PURPLE", [154, 101, 231, 255]),
    ("PINK", [246, 106, 164, 255]),
    ("WHITE", [248, 250, 255, 255]),
    ("GRAY", [137, 146, 165, 255]),
    ("BLACK", [22, 26, 34, 255]),
];

pub(crate) fn color_menu_layout(point: (f64, f64), screen: PixelSize) -> ContextMenu {
    let mut menu = context_menu_layout(EditorTool::Pen, point, screen);
    // Both palette and custom picker share the same on-screen origin. Otherwise,
    // opening the taller picker near the bottom shifts it upward under the cursor.
    let custom_bounds = popup_bounds(point, screen, CONTEXT_MENU_SIZE);
    let offset_x = custom_bounds.x - menu.bounds.x;
    let offset_y = custom_bounds.y - menu.bounds.y;
    menu.bounds.x += offset_x;
    menu.bounds.y += offset_y;
    menu.kind = ContextMenuKind::Color;
    menu.options = (0..PALETTE.len())
        .map(|index| {
            let col = index % 2;
            let row = index / 2;
            (
                index as i32,
                RectI {
                    x: menu.bounds.x + 7 + col as i32 * 89,
                    y: menu.bounds.y + 32 + row as i32 * 36,
                    width: 84,
                    height: 34,
                },
            )
        })
        .collect();
    menu.options.push((
        PALETTE.len() as i32,
        RectI { x: menu.bounds.x + 7, y: menu.bounds.y + 32 + 6 * 36, width: menu.bounds.width - 14, height: 34 },
    ));
    menu
}

fn popup_bounds(point: (f64, f64), screen: PixelSize, size: PixelSize) -> RectI {
    let width = size.width as i32;
    let height = size.height as i32;
    let x = (point.0.round() as i32).clamp(0, (i32::try_from(screen.width).unwrap_or(i32::MAX) - width).max(0));
    let y = (point.1.round() as i32).clamp(0, (i32::try_from(screen.height).unwrap_or(i32::MAX) - height).max(0));
    RectI { x, y, width, height }
}

pub(crate) fn custom_color_menu_layout(point: (f64, f64), screen: PixelSize) -> ContextMenu {
    let bounds = popup_bounds(point, screen, CONTEXT_MENU_SIZE);
    let x = bounds.x;
    let y = bounds.y;
    let options = vec![
        (0, RectI { x: x + 14, y: y + 228, width: 216, height: 28 }),
        (1, RectI { x: x + 14, y: y + 262, width: 216, height: 28 }),
        (2, RectI { x: x + 14, y: y + 296, width: 216, height: 28 }),
        (3, RectI { x: x + 14, y: y + 332, width: 216, height: 32 }),
        (4, RectI { x: x + 14, y: y + 391, width: 104, height: 34 }),
        (5, RectI { x: x + 126, y: y + 391, width: 104, height: 34 }),
        (CUSTOM_SV_ID, RectI { x: x + 14, y: y + 32, width: SV_WIDTH as i32, height: SV_HEIGHT as i32 }),
        (
            CUSTOM_HUE_ID,
            RectI { x: x + 14, y: y + 168, width: HUE_STRIP_WIDTH as i32, height: HUE_STRIP_HEIGHT as i32 },
        ),
    ];
    ContextMenu { kind: ContextMenuKind::CustomColor, bounds, options }
}

pub(crate) fn custom_sat_value_at(menu: &ContextMenu, point: (f64, f64)) -> Option<(f64, f64)> {
    if menu.kind != ContextMenuKind::CustomColor {
        return None;
    }
    let rect = menu.options.iter().find(|(id, _)| *id == CUSTOM_SV_ID)?.1;
    // Align pointer samples to the exact displayed pixel grid/marker, avoiding
    // redraws for subpixel motion that cannot change the selected palette pixel.
    let px = (point.0 - f64::from(rect.x)).round().clamp(0.0, f64::from(rect.width - 1));
    let py = (point.1 - f64::from(rect.y)).round().clamp(0.0, f64::from(rect.height - 1));
    let saturation = px / f64::from(rect.width - 1);
    let value = 1.0 - py / f64::from(rect.height - 1);
    Some((saturation, value))
}

pub(crate) fn custom_hue_at(menu: &ContextMenu, pointer_x: f64) -> Option<f64> {
    if menu.kind != ContextMenuKind::CustomColor {
        return None;
    }
    let rect = menu.options.iter().find(|(id, _)| *id == CUSTOM_HUE_ID)?.1;
    let pixel = (pointer_x - f64::from(rect.x)).round().clamp(0.0, f64::from(rect.width - 1));
    Some(pixel / f64::from(rect.width - 1) * 360.0)
}

pub(crate) fn custom_slider_value(menu: &ContextMenu, channel: usize, pointer_x: f64) -> Option<u8> {
    if menu.kind != ContextMenuKind::CustomColor || channel > 2 {
        return None;
    }
    let rect = menu.options.iter().find(|(value, _)| *value == channel as i32)?.1;
    let start = f64::from(rect.x + 68);
    let end = f64::from(rect.x + rect.width - 10);
    Some(((pointer_x - start) / (end - start) * 255.0).round().clamp(0.0, 255.0) as u8)
}

pub(crate) fn parse_hex_color(input: &str) -> Option<[u8; 4]> {
    let hex = input.strip_prefix('#').unwrap_or(input);
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
        255,
    ])
}

pub(crate) fn hex_color(color: [u8; 4]) -> String {
    format!("{:02X}{:02X}{:02X}", color[0], color[1], color[2])
}

pub(crate) fn rgb_to_hsv(color: [u8; 4]) -> (f64, f64, f64) {
    let r = f64::from(color[0]) / 255.0;
    let g = f64::from(color[1]) / 255.0;
    let b = f64::from(color[2]) / 255.0;
    let maximum = r.max(g).max(b);
    let minimum = r.min(g).min(b);
    let delta = maximum - minimum;
    let hue = if delta <= f64::EPSILON {
        0.0
    } else if maximum == r {
        (60.0 * ((g - b) / delta)).rem_euclid(360.0)
    } else if maximum == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if maximum <= f64::EPSILON { 0.0 } else { delta / maximum };
    (hue, saturation, maximum)
}

pub(crate) fn hsv_to_rgb(hue: f64, saturation: f64, value: f64) -> [u8; 4] {
    let h = hue.rem_euclid(360.0) / 60.0;
    let saturation = saturation.clamp(0.0, 1.0);
    let value = value.clamp(0.0, 1.0);
    let chroma = value * saturation;
    let secondary = chroma * (1.0 - ((h % 2.0) - 1.0).abs());
    let minimum = value - chroma;
    let (r, g, b) = match h as u8 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let channel = |value: f64| ((value + minimum) * 255.0).round().clamp(0.0, 255.0) as u8;
    [channel(r), channel(g), channel(b), 255]
}

fn hue_strip_pixels() -> &'static [u8] {
    static HUE_STRIP: OnceLock<Vec<u8>> = OnceLock::new();
    HUE_STRIP.get_or_init(|| {
        let mut pixels = vec![0; HUE_STRIP_WIDTH * HUE_STRIP_HEIGHT * 4];
        for column in 0..HUE_STRIP_WIDTH {
            let color = hsv_to_rgb(360.0 * column as f64 / (HUE_STRIP_WIDTH - 1) as f64, 1.0, 1.0);
            for row in 0..HUE_STRIP_HEIGHT {
                let offset = (row * HUE_STRIP_WIDTH + column) * 4;
                pixels[offset..offset + 4].copy_from_slice(&color);
            }
        }
        pixels
    })
}

fn create_sv_gradient(hue: f64) -> Arc<[u8]> {
    let pure = hsv_to_rgb(hue, 1.0, 1.0);
    let mut pixels = vec![0_u8; SV_WIDTH * SV_HEIGHT * 4];
    let width_steps = (SV_WIDTH - 1) as u32;
    let height_steps = (SV_HEIGHT - 1) as u32;
    let denominator = width_steps * height_steps;
    for row in 0..SV_HEIGHT {
        let brightness = height_steps - row as u32;
        for column in 0..SV_WIDTH {
            let saturation = column as u32;
            let offset = (row * SV_WIDTH + column) * 4;
            for channel in 0..3 {
                // The gradient depends on hue only: precompute it once and reuse
                // the RGBA rows for every saturation/value pointer movement.
                let top = 255 * (width_steps - saturation) + u32::from(pure[channel]) * saturation;
                pixels[offset + channel] = ((top * brightness + denominator / 2) / denominator) as u8;
            }
            pixels[offset + 3] = 255;
        }
    }
    pixels.into()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CustomColorEditor {
    pub(crate) color: [u8; 4],
    pub(crate) hex_input: String,
    pub(crate) editing_hex: bool,
    // 0..2 = RGB sliders, 3 = saturation/value square, 4 = hue strip.
    pub(crate) dragging_channel: Option<usize>,
    pub(crate) hue: f64,
    pub(crate) saturation: f64,
    pub(crate) value: f64,
    sv_gradient: Arc<[u8]>,
}

impl CustomColorEditor {
    pub(crate) fn new(color: [u8; 4]) -> Self {
        let (hue, saturation, value) = rgb_to_hsv(color);
        Self {
            color,
            hex_input: hex_color(color),
            editing_hex: false,
            dragging_channel: None,
            hue,
            saturation,
            value,
            sv_gradient: create_sv_gradient(hue),
        }
    }

    fn sync_hsv_from_rgb(&mut self) {
        let (hue, saturation, value) = rgb_to_hsv(self.color);
        // Preserve hue at zero saturation/brightness so the SV rectangle remains useful.
        if saturation > 0.0 && value > 0.0 && self.hue != hue {
            self.hue = hue;
            self.sv_gradient = create_sv_gradient(hue);
        }
        self.saturation = saturation;
        self.value = value;
    }

    fn sync_rgb_from_hsv(&mut self) {
        self.color = hsv_to_rgb(self.hue, self.saturation, self.value);
        self.hex_input = hex_color(self.color);
        self.editing_hex = false;
    }

    pub(crate) fn set_channel(&mut self, channel: usize, value: u8) -> bool {
        let Some(component) = self.color.get_mut(channel) else {
            return false;
        };
        let changed = *component != value || self.editing_hex;
        *component = value;
        self.hex_input = hex_color(self.color);
        self.editing_hex = false;
        self.sync_hsv_from_rgb();
        changed
    }

    pub(crate) fn set_saturation_value(&mut self, saturation: f64, value: f64) -> bool {
        let saturation = saturation.clamp(0.0, 1.0);
        let value = value.clamp(0.0, 1.0);
        let changed = self.saturation != saturation || self.value != value || self.editing_hex;
        self.saturation = saturation;
        self.value = value;
        self.sync_rgb_from_hsv();
        changed
    }

    pub(crate) fn set_hue(&mut self, hue: f64) -> bool {
        let hue = hue.clamp(0.0, 360.0);
        let changed = self.hue != hue || self.editing_hex;
        if self.hue != hue {
            self.hue = hue;
            self.sv_gradient = create_sv_gradient(hue);
        }
        self.sync_rgb_from_hsv();
        changed
    }

    pub(crate) fn start_hex_input(&mut self) {
        self.hex_input.clear();
        self.editing_hex = true;
        self.dragging_channel = None;
    }

    pub(crate) fn push_hex(&mut self, ch: char) -> bool {
        if !self.editing_hex || !ch.is_ascii_hexdigit() || self.hex_input.len() >= 6 {
            return false;
        }
        self.hex_input.push(ch.to_ascii_uppercase());
        if let Some(color) = parse_hex_color(&self.hex_input) {
            self.color = color;
            self.sync_hsv_from_rgb();
        }
        true
    }

    pub(crate) fn backspace_hex(&mut self) -> bool {
        self.editing_hex && self.hex_input.pop().is_some()
    }

    pub(crate) fn chosen_color(&self) -> Option<[u8; 4]> {
        if self.editing_hex { parse_hex_color(&self.hex_input) } else { Some(self.color) }
    }
}

pub(crate) fn context_menu_layout(tool: EditorTool, point: (f64, f64), screen: PixelSize) -> ContextMenu {
    const STROKE_SIZES: [i32; 6] = [1, 2, 3, 5, 8, 12];
    const FONT_SIZES: [i32; 6] = [16, 20, 24, 32, 40, 48];
    const MOSAIC_SIZES: [i32; 6] = [8, 12, 18, 24, 32, 48];
    const ROW_HEIGHT: i32 = 36;
    const HEADER_HEIGHT: i32 = 32;
    let width = STANDARD_MENU_WIDTH;
    let height = STANDARD_MENU_HEIGHT;
    let kind = match tool {
        EditorTool::Text => ContextMenuKind::Font,
        EditorTool::Mosaic => ContextMenuKind::Mosaic,
        _ => ContextMenuKind::Stroke,
    };
    let values = match kind {
        ContextMenuKind::Stroke => STROKE_SIZES,
        ContextMenuKind::Font => FONT_SIZES,
        ContextMenuKind::Color | ContextMenuKind::CustomColor => STROKE_SIZES,
        ContextMenuKind::Mosaic => MOSAIC_SIZES,
    };
    let screen_width = i32::try_from(screen.width).unwrap_or(i32::MAX);
    let screen_height = i32::try_from(screen.height).unwrap_or(i32::MAX);
    let x = (point.0.round() as i32).clamp(0, (screen_width - width).max(0));
    let y = (point.1.round() as i32).clamp(0, (screen_height - height).max(0));
    let bounds = RectI { x, y, width, height };
    let options = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            (
                value,
                RectI {
                    x: x + 7,
                    y: y + HEADER_HEIGHT + index as i32 * ROW_HEIGHT,
                    width: width - 14,
                    height: ROW_HEIGHT - 2,
                },
            )
        })
        .collect();
    ContextMenu { kind, bounds, options }
}

const TOOLBAR_BUTTON: i32 = 40;
const TOOLBAR_GAP: i32 = 6;
const TOOLBAR_PAD: i32 = 8;
const TOOLBAR_HEIGHT: i32 = TOOLBAR_BUTTON + TOOLBAR_PAD * 2;
const TOOLBAR_COUNT: i32 = 10;

const TEXT_SELECTION_COLOR: [u8; 4] = [77, 139, 233, 185];
const TOOLBAR_BG: [u8; 4] = [24, 27, 32, 232];
const TOOLBAR_BUTTON_BG: [u8; 4] = [49, 54, 64, 236];
const TOOLBAR_ACTIVE_BG: [u8; 4] = [67, 92, 120, 244];
const TOOLBAR_ICON: [u8; 4] = [242, 245, 248, 255];
const TOOLBAR_CONFIRM: [u8; 4] = [93, 201, 128, 255];
const TOOLBAR_CANCEL: [u8; 4] = [242, 102, 102, 255];
const MENU_SELECTED_BG: [u8; 4] = [66, 113, 166, 244];
const ICON_VIEWBOX: f64 = 24.0;

static SYSTEM_FONT: OnceLock<Font> = OnceLock::new();
static SYSTEM_FONT_LOADING: AtomicBool = AtomicBool::new(false);

const PEN_ICON: &str = include_str!("../../../../../res/icons/toolbar/pen.svg");
const RECTANGLE_ICON: &str = include_str!("../../../../../res/icons/toolbar/rectangle.svg");
const ARROW_ICON: &str = include_str!("../../../../../res/icons/toolbar/arrow.svg");
const TEXT_ICON: &str = include_str!("../../../../../res/icons/toolbar/text.svg");
const MOSAIC_ICON: &str = include_str!("../../../../../res/icons/toolbar/mosaic.svg");
const UNDO_ICON: &str = include_str!("../../../../../res/icons/toolbar/undo.svg");
const REDO_ICON: &str = include_str!("../../../../../res/icons/toolbar/redo.svg");
const CONFIRM_ICON: &str = include_str!("../../../../../res/icons/toolbar/confirm.svg");
const CANCEL_ICON: &str = include_str!("../../../../../res/icons/toolbar/cancel.svg");

pub(crate) fn normalized_rect(start: (f64, f64), end: (f64, f64), bounds: PixelSize) -> Option<RectI> {
    let width = i32::try_from(bounds.width).ok()?;
    let height = i32::try_from(bounds.height).ok()?;
    let x1 = start.0.min(end.0).floor().clamp(0.0, f64::from(width)) as i32;
    let y1 = start.1.min(end.1).floor().clamp(0.0, f64::from(height)) as i32;
    let x2 = start.0.max(end.0).ceil().clamp(0.0, f64::from(width)) as i32;
    let y2 = start.1.max(end.1).ceil().clamp(0.0, f64::from(height)) as i32;
    (x2 > x1 && y2 > y1).then_some(RectI { x: x1, y: y1, width: x2 - x1, height: y2 - y1 })
}

pub(crate) fn toolbar_layout(selection: RectI, screen: PixelSize) -> ToolbarLayout {
    let width = TOOLBAR_PAD * 2 + TOOLBAR_COUNT * TOOLBAR_BUTTON + (TOOLBAR_COUNT - 1) * TOOLBAR_GAP;
    let screen_width = i32::try_from(screen.width).unwrap_or(i32::MAX);
    let screen_height = i32::try_from(screen.height).unwrap_or(i32::MAX);
    let center = selection.x + selection.width / 2;
    let x = (center - width / 2).clamp(8, (screen_width - width - 8).max(8));
    let below = selection.y + selection.height + 12;
    let above = selection.y - TOOLBAR_HEIGHT - 12;
    let y = if below + TOOLBAR_HEIGHT <= screen_height - 8 {
        below
    } else if above >= 8 {
        above
    } else {
        (screen_height - TOOLBAR_HEIGHT - 8).max(8)
    };
    let bounds = RectI { x, y, width, height: TOOLBAR_HEIGHT };
    let actions = [
        ToolbarAction::Color,
        ToolbarAction::Tool(EditorTool::Pen),
        ToolbarAction::Tool(EditorTool::Rectangle),
        ToolbarAction::Tool(EditorTool::Arrow),
        ToolbarAction::Tool(EditorTool::Text),
        ToolbarAction::Tool(EditorTool::Mosaic),
        ToolbarAction::Undo,
        ToolbarAction::Redo,
        ToolbarAction::Confirm,
        ToolbarAction::Cancel,
    ];
    let buttons = actions
        .into_iter()
        .enumerate()
        .map(|(index, action)| {
            let bx = x + TOOLBAR_PAD + i32::try_from(index).unwrap_or(0) * (TOOLBAR_BUTTON + TOOLBAR_GAP);
            (action, RectI { x: bx, y: y + TOOLBAR_PAD, width: TOOLBAR_BUTTON, height: TOOLBAR_BUTTON })
        })
        .collect();
    ToolbarLayout { bounds, buttons }
}

impl ToolbarLayout {
    pub(crate) fn hit_test(&self, point: (f64, f64)) -> Option<ToolbarAction> {
        self.buttons.iter().find_map(|(action, rect)| rect.contains(point).then_some(*action))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MosaicSource {
    pub(crate) image: RgbaImage,
    pub(crate) logical_size: PixelSize,
}

#[cfg(test)]
pub(crate) fn render_annotations_patch(rect: RectI, annotations: &[Annotation]) -> Vec<u8> {
    render_annotations_patch_with_source(rect, annotations, None)
}

pub(crate) fn render_annotations_patch_with_source(
    rect: RectI,
    annotations: &[Annotation],
    source: Option<&MosaicSource>,
) -> Vec<u8> {
    let mut raster = patch_raster(rect);
    for annotation in annotations {
        draw_annotation(&mut raster, annotation, 1.0, 1.0, source);
    }
    finish_patch(raster)
}

#[cfg(test)]
pub(crate) fn render_draft_patch(
    rect: RectI,
    draft: Option<&DraftAnnotation>,
    text_draft: Option<&TextDraft>,
    cursor_visible: bool,
) -> Vec<u8> {
    render_draft_patch_with_move(rect, draft, text_draft, cursor_visible, None, None, None)
}

pub(crate) fn render_draft_patch_with_move(
    rect: RectI,
    draft: Option<&DraftAnnotation>,
    text_draft: Option<&TextDraft>,
    cursor_visible: bool,
    moving: Option<&Annotation>,
    movable: Option<&Annotation>,
    source: Option<&MosaicSource>,
) -> Vec<u8> {
    let mut raster = patch_raster(rect);
    if let Some(draft) = draft {
        draw_draft(&mut raster, draft, 1.0, 1.0, source);
    }
    if let Some(moving) = moving {
        if let Annotation::Mosaic { start, end, .. } = moving {
            draw_draft(
                &mut raster,
                &DraftAnnotation::Mosaic { start: *start, end: *end, block_size: 18 },
                1.0,
                1.0,
                source,
            );
        } else {
            draw_annotation(&mut raster, moving, 1.0, 1.0, source);
        }
        draw_selected_annotation(&mut raster, moving);
    }
    if let Some(text) = text_draft {
        let rendered = format!("{}{}", text.text, text.preedit);
        if text.selected_all {
            draw_text_selection(&mut raster, text.position, &text.text, text.font_size, 1.0, 1.0);
        }
        draw_text(&mut raster, text.position, &rendered, text.font_size, text.color, 1.0, 1.0);
        if cursor_visible && !text.selected_all {
            draw_text_cursor(&mut raster, text.position, &rendered, text.font_size, text.color, 1.0, 1.0);
        }
    }
    if let Some(annotation) = movable {
        draw_selected_annotation(&mut raster, annotation);
    }
    finish_patch(raster)
}

pub(crate) fn render_toolbar_patch_with_color(toolbar: &ToolbarLayout, tool: EditorTool, color: [u8; 4]) -> Vec<u8> {
    let mut raster = patch_raster(toolbar.bounds);
    raster.clip = None;
    draw_toolbar(&mut raster, toolbar, tool, color);
    finish_patch(raster)
}

pub(crate) fn render_context_menu_patch(
    menu: &ContextMenu,
    selected: i32,
    custom_color: [u8; 4],
    editor: Option<&CustomColorEditor>,
) -> Vec<u8> {
    // Keep a fixed-size transparent backing for every menu to match the Wayland SHM layer.
    let mut raster = patch_raster(RectI {
        x: menu.bounds.x,
        y: menu.bounds.y,
        width: CONTEXT_MENU_SIZE.width as i32,
        height: CONTEXT_MENU_SIZE.height as i32,
    });
    // The backing patch starts clear, so writing the translucent backdrop
    // directly avoids 100k+ unnecessary alpha-compositing operations per drag.
    raster.fill_rect_replace(menu.bounds, TOOLBAR_BG);
    if menu.kind == ContextMenuKind::CustomColor {
        if let Some(editor) = editor {
            draw_custom_color_picker(&mut raster, menu, editor);
        }
        return finish_patch(raster);
    }
    let label = match menu.kind {
        ContextMenuKind::Stroke => "LINE WIDTH",
        ContextMenuKind::Font => "FONT SIZE",
        ContextMenuKind::Color => "DRAW COLOR",
        ContextMenuKind::Mosaic => "MOSAIC BLOCK",
        ContextMenuKind::CustomColor => unreachable!(),
    };
    raster.draw_text_bitmap((menu.bounds.x + 12, menu.bounds.y + 8), label, 2, TOOLBAR_ICON);
    for (value, rect) in &menu.options {
        raster.fill_rect(*rect, if *value == selected { MENU_SELECTED_BG } else { TOOLBAR_BUTTON_BG });
        if menu.kind == ContextMenuKind::Color {
            if let Some((name, color)) = PALETTE.get(*value as usize) {
                raster.fill_rect(RectI { x: rect.x + 6, y: rect.y + 6, width: 20, height: 22 }, *color);
                raster.draw_text_bitmap((rect.x + 31, rect.y + 13), name, 1, TOOLBAR_ICON);
            } else if *value == PALETTE.len() as i32 {
                raster.fill_rect(RectI { x: rect.x + 8, y: rect.y + 5, width: 25, height: 24 }, custom_color);
                raster.draw_text_bitmap((rect.x + 42, rect.y + 10), "CUSTOM...", 2, TOOLBAR_ICON);
            }
            continue;
        }
        match menu.kind {
            ContextMenuKind::Stroke => raster.draw_line(
                (rect.x + 16, rect.y + rect.height / 2),
                (rect.x + 72, rect.y + rect.height / 2),
                *value,
                TOOLBAR_ICON,
            ),
            ContextMenuKind::Font => raster.draw_text_bitmap((rect.x + 16, rect.y + 8), "Aa", 2, TOOLBAR_ICON),
            ContextMenuKind::Mosaic => raster.draw_text_bitmap((rect.x + 16, rect.y + 8), "####", 2, TOOLBAR_ICON),
            ContextMenuKind::Color | ContextMenuKind::CustomColor => {}
        }
        raster.draw_text_bitmap((rect.x + 96, rect.y + 8), &format!("{value} px"), 2, TOOLBAR_ICON);
    }
    finish_patch(raster)
}

fn draw_custom_color_picker(raster: &mut Raster, menu: &ContextMenu, editor: &CustomColorEditor) {
    let x = menu.bounds.x;
    let y = menu.bounds.y;
    raster.draw_text_bitmap((x + 14, y + 9), "CUSTOM COLOR", 2, TOOLBAR_ICON);

    // Saturation increases left-to-right, brightness decreases top-to-bottom.
    if let Some((_, square)) = menu.options.iter().find(|(id, _)| *id == CUSTOM_SV_ID) {
        raster.blit_rgba(*square, &editor.sv_gradient);
        let px = square.x + (editor.saturation * f64::from(square.width - 1)).round() as i32;
        let py = square.y + ((1.0 - editor.value) * f64::from(square.height - 1)).round() as i32;
        raster.draw_disc(px, py, 6, [0, 0, 0, 255]);
        raster.draw_disc(px, py, 4, [255, 255, 255, 255]);
        raster.draw_disc(px, py, 2, editor.color);
    }

    // A rainbow hue strip selects the base hue for the 2D square.
    if let Some((_, strip)) = menu.options.iter().find(|(id, _)| *id == CUSTOM_HUE_ID) {
        raster.blit_rgba(*strip, hue_strip_pixels());
        let marker = strip.x + (editor.hue / 360.0 * f64::from(strip.width - 1)).round() as i32;
        raster.fill_rect(RectI { x: marker - 2, y: strip.y - 2, width: 5, height: strip.height + 4 }, [0, 0, 0, 255]);
        raster.fill_rect(RectI { x: marker, y: strip.y - 1, width: 1, height: strip.height + 2 }, [255, 255, 255, 255]);
    }

    // Full-width live preview, then the existing RGB/HEX and confirmation controls.
    let luminance =
        (u32::from(editor.color[0]) * 299 + u32::from(editor.color[1]) * 587 + u32::from(editor.color[2]) * 114) / 1000;
    let contrasting = if luminance >= 148 { [20, 25, 33, 255] } else { [250, 252, 255, 255] };
    raster.fill_rect(RectI { x: x + 14, y: y + 191, width: 216, height: 28 }, editor.color);
    raster.draw_text_bitmap((x + 20, y + 198), &format!("#{}", hex_color(editor.color)), 2, contrasting);

    for (id, rect) in &menu.options {
        if *id == CUSTOM_SV_ID || *id == CUSTOM_HUE_ID {
            continue;
        }
        let background = if *id == 3 && editor.editing_hex { MENU_SELECTED_BG } else { TOOLBAR_BUTTON_BG };
        raster.fill_rect(*rect, background);
        match *id {
            0..=2 => {
                let channel = *id as usize;
                let value = editor.color[channel];
                let label = ["R", "G", "B"][channel];
                raster.draw_text_bitmap((rect.x + 8, rect.y + 8), label, 2, TOOLBAR_ICON);
                raster.draw_text_bitmap((rect.x + 26, rect.y + 10), &format!("{value:03}"), 1, TOOLBAR_ICON);
                let track_start = rect.x + 68;
                let track_end = rect.x + rect.width - 10;
                let track_y = rect.y + 14;
                raster.fill_rect(
                    RectI { x: track_start, y: track_y - 2, width: track_end - track_start + 1, height: 4 },
                    [88, 96, 110, 255],
                );
                let handle_x =
                    track_start + ((track_end - track_start) as f64 * f64::from(value) / 255.0).round() as i32;
                raster.fill_rect(RectI { x: handle_x - 3, y: track_y - 7, width: 7, height: 14 }, TOOLBAR_ICON);
            }
            3 => {
                raster.draw_text_bitmap((rect.x + 8, rect.y + 12), "HEX", 1, TOOLBAR_ICON);
                let value = if editor.editing_hex {
                    format!("#{}_", editor.hex_input)
                } else {
                    format!("#{}", hex_color(editor.color))
                };
                raster.draw_text_bitmap((rect.x + 52, rect.y + 7), &value, 2, TOOLBAR_ICON);
            }
            4 => raster.draw_text_bitmap((rect.x + 15, rect.y + 11), "APPLY", 2, TOOLBAR_ICON),
            5 => raster.draw_text_bitmap((rect.x + 21, rect.y + 11), "BACK", 2, TOOLBAR_ICON),
            _ => {}
        }
    }
    if editor.editing_hex && editor.chosen_color().is_none() {
        raster.draw_text_bitmap((x + 16, y + 374), "ENTER 6 HEX DIGITS", 1, [245, 158, 106, 255]);
    }
}

pub(crate) fn draft_damage_regions(
    draft: Option<&DraftAnnotation>,
    text_draft: Option<&TextDraft>,
    bounds: RectI,
) -> Vec<RectI> {
    let mut regions = Vec::new();
    if let Some(draft) = draft {
        match draft {
            DraftAnnotation::Pen { points, width, .. } => {
                if points.len() == 1 {
                    let point = transform_point(points[0], 1.0, 1.0);
                    let margin = (*width + 1) / 2 + 2;
                    regions.push(RectI {
                        x: point.0 - margin,
                        y: point.1 - margin,
                        width: margin * 2 + 1,
                        height: margin * 2 + 1,
                    });
                } else {
                    for pair in points.windows(2) {
                        regions.push(line_damage(pair[0], pair[1], (*width + 1) / 2 + 2));
                    }
                }
            }
            DraftAnnotation::Rectangle { start, end, width, .. } => {
                if let Some(rect) = rect_from_points(transform_point(*start, 1.0, 1.0), transform_point(*end, 1.0, 1.0))
                {
                    let margin = (*width + 1) / 2 + 2;
                    regions.extend([
                        RectI {
                            x: rect.x - margin,
                            y: rect.y - margin,
                            width: rect.width + margin * 2 + 1,
                            height: margin * 2 + 1,
                        },
                        RectI {
                            x: rect.x - margin,
                            y: rect.y + rect.height - margin,
                            width: rect.width + margin * 2 + 1,
                            height: margin * 2 + 1,
                        },
                        RectI {
                            x: rect.x - margin,
                            y: rect.y - margin,
                            width: margin * 2 + 1,
                            height: rect.height + margin * 2 + 1,
                        },
                        RectI {
                            x: rect.x + rect.width - margin,
                            y: rect.y - margin,
                            width: margin * 2 + 1,
                            height: rect.height + margin * 2 + 1,
                        },
                    ]);
                }
            }
            DraftAnnotation::Arrow { start, end, width, .. } => {
                regions.extend(arrow_damage_regions(*start, *end, *width));
            }
            DraftAnnotation::Mosaic { start, end, .. } => {
                if let Some(rect) = rect_from_points(transform_point(*start, 1.0, 1.0), transform_point(*end, 1.0, 1.0))
                {
                    regions.push(RectI {
                        x: rect.x - 3,
                        y: rect.y - 3,
                        width: rect.width + 6,
                        height: rect.height + 6,
                    });
                }
            }
        }
    }
    if let Some(text) = text_draft {
        regions.push(text_damage_rect(text));
    }
    regions.into_iter().filter_map(|rect| intersect_rect(rect, bounds)).collect()
}

fn patch_raster(rect: RectI) -> Raster {
    let width = u32::try_from(rect.width.max(0)).unwrap_or(0);
    let height = u32::try_from(rect.height.max(0)).unwrap_or(0);
    Raster {
        size: PixelSize { width, height },
        origin: (rect.x, rect.y),
        pixels: vec![0; width as usize * height as usize * 4],
        clip: Some(rect),
    }
}

fn finish_patch(mut raster: Raster) -> Vec<u8> {
    rgba_to_wayland_argb_in_place(&mut raster.pixels);
    raster.pixels
}

fn line_damage(start: (f64, f64), end: (f64, f64), margin: i32) -> RectI {
    let start = transform_point(start, 1.0, 1.0);
    let end = transform_point(end, 1.0, 1.0);
    let x1 = start.0.min(end.0) - margin;
    let y1 = start.1.min(end.1) - margin;
    let x2 = start.0.max(end.0) + margin;
    let y2 = start.1.max(end.1) + margin;
    RectI { x: x1, y: y1, width: x2 - x1 + 1, height: y2 - y1 + 1 }
}

fn arrow_damage_regions(start: (f64, f64), end: (f64, f64), width: i32) -> Vec<RectI> {
    let margin = (width + 1) / 2 + 2;
    let mut regions = vec![line_damage(start, end, margin)];
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 {
        return regions;
    }
    let ux = dx / length;
    let uy = dy / length;
    let head = 14.0_f64.max(f64::from(width) * 4.0);
    let side = head * 0.55;
    let base = (end.0 - ux * head, end.1 - uy * head);
    let left = (base.0 - uy * side, base.1 + ux * side);
    let right = (base.0 + uy * side, base.1 - ux * side);
    regions.push(line_damage(end, left, margin));
    regions.push(line_damage(end, right, margin));
    regions
}

fn text_damage_rect(text: &TextDraft) -> RectI {
    let rendered = format!("{}{}", text.text, text.preedit);
    let lines = rendered.split('\n').collect::<Vec<_>>();
    let columns = lines.iter().map(|line| line.chars().count()).max().unwrap_or(1).max(1);
    let rows = lines.len().max(1);
    RectI {
        x: text.position.0.floor() as i32 - 3,
        y: text.position.1.floor() as i32 - 3,
        width: i32::try_from(columns.saturating_add(1))
            .unwrap_or(i32::MAX)
            .saturating_mul(text.font_size + 8)
            .saturating_add(8),
        height: i32::try_from(rows).unwrap_or(i32::MAX).saturating_mul(text.font_size + 8).saturating_add(8),
    }
}

fn intersect_rect(rect: RectI, bounds: RectI) -> Option<RectI> {
    let x1 = rect.x.max(bounds.x);
    let y1 = rect.y.max(bounds.y);
    let x2 = (rect.x + rect.width).min(bounds.x + bounds.width);
    let y2 = (rect.y + rect.height).min(bounds.y + bounds.height);
    (x2 > x1 && y2 > y1).then_some(RectI { x: x1, y: y1, width: x2 - x1, height: y2 - y1 })
}

pub(crate) fn compose_annotated_crop(
    frame: &CaptureFrame,
    logical_size: PixelSize,
    selection: PixelRect,
    annotations: &[Annotation],
) -> Result<RgbaImage, CaptureError> {
    let frame_size = frame.image.size();
    let mut raster = Raster { size: frame_size, origin: (0, 0), pixels: frame.image.pixels().to_vec(), clip: None };
    let sx = f64::from(frame_size.width) / f64::from(logical_size.width);
    let sy = f64::from(frame_size.height) / f64::from(logical_size.height);
    raster.clip = Some(RectI {
        x: i32::try_from(selection.x).unwrap_or(i32::MAX),
        y: i32::try_from(selection.y).unwrap_or(i32::MAX),
        width: i32::try_from(selection.width).unwrap_or(i32::MAX),
        height: i32::try_from(selection.height).unwrap_or(i32::MAX),
    });
    let source = MosaicSource { image: frame.image.clone(), logical_size };
    for annotation in annotations {
        draw_annotation(&mut raster, annotation, sx, sy, Some(&source));
    }
    RgbaImage::new(frame_size, raster.pixels)?.crop(selection)
}

pub(crate) fn annotation_hit_test(annotation: &Annotation, point: (f64, f64)) -> bool {
    match annotation {
        Annotation::Pen { points, width, .. } => {
            let tolerance = f64::from(*width) / 2.0 + 6.0;
            points.windows(2).any(|pair| point_segment_distance(point, pair[0], pair[1]) <= tolerance)
                || points.len() == 1 && point_distance(point, points[0]) <= tolerance
        }
        Annotation::Arrow { start, end, width, .. } => {
            let tolerance = f64::from(*width) / 2.0 + 6.0;
            if point_segment_distance(point, *start, *end) <= tolerance {
                return true;
            }
            let dx = end.0 - start.0;
            let dy = end.1 - start.1;
            let length = dx.hypot(dy);
            if length <= 0.0 {
                return false;
            }
            let ux = dx / length;
            let uy = dy / length;
            let head = 14.0_f64.max(f64::from(*width) * 4.0);
            let side = head * 0.55;
            let base = (end.0 - ux * head, end.1 - uy * head);
            [(base.0 - uy * side, base.1 + ux * side), (base.0 + uy * side, base.1 - ux * side)]
                .into_iter()
                .any(|tip| point_segment_distance(point, *end, tip) <= tolerance)
        }
        Annotation::Rectangle { start, end, width, .. } => {
            let x1 = start.0.min(end.0);
            let x2 = start.0.max(end.0);
            let y1 = start.1.min(end.1);
            let y2 = start.1.max(end.1);
            let tolerance = f64::from(*width) / 2.0 + 5.0;
            point.0 >= x1 - tolerance
                && point.0 <= x2 + tolerance
                && point.1 >= y1 - tolerance
                && point.1 <= y2 + tolerance
        }
        Annotation::Text { position, text, font_size, .. } => {
            let px = *font_size as f32;
            let width = text
                .lines()
                .map(|line| {
                    system_font().map_or(line.chars().count() as f32 * px * 0.8, |font| {
                        line.chars().map(|ch| font.metrics(ch, px).advance_width).sum()
                    })
                })
                .fold(0.0_f32, f32::max);
            let height = text.lines().count() as f64 * (f64::from(*font_size) + 4.0);
            point.0 >= position.0 - 5.0
                && point.0 <= position.0 + f64::from(width) + 5.0
                && point.1 >= position.1 - 5.0
                && point.1 <= position.1 + height + 5.0
        }
        Annotation::Mosaic { start, end, .. } => {
            point.0 >= start.0.min(end.0)
                && point.0 <= start.0.max(end.0)
                && point.1 >= start.1.min(end.1)
                && point.1 <= start.1.max(end.1)
        }
    }
}

pub(crate) fn annotation_bounds(annotation: &Annotation) -> RectI {
    match annotation {
        Annotation::Pen { points, width, .. } => {
            let margin = *width + 4;
            let min_x = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
            let min_y = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let max_x = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
            let max_y = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            if points.is_empty() {
                return RectI { x: 0, y: 0, width: 0, height: 0 };
            }
            RectI {
                x: min_x.floor() as i32 - margin,
                y: min_y.floor() as i32 - margin,
                width: (max_x.ceil() - min_x.floor()) as i32 + margin * 2 + 1,
                height: (max_y.ceil() - min_y.floor()) as i32 + margin * 2 + 1,
            }
        }
        Annotation::Rectangle { start, end, width, .. } | Annotation::Arrow { start, end, width, .. } => {
            let margin = if matches!(annotation, Annotation::Arrow { .. }) {
                14_i32.max(*width * 4) + *width + 3
            } else {
                *width + 3
            };
            RectI {
                x: start.0.min(end.0).floor() as i32 - margin,
                y: start.1.min(end.1).floor() as i32 - margin,
                width: (start.0.max(end.0).ceil() - start.0.min(end.0).floor()) as i32 + margin * 2 + 1,
                height: (start.1.max(end.1).ceil() - start.1.min(end.1).floor()) as i32 + margin * 2 + 1,
            }
        }
        Annotation::Text { position, text, font_size, .. } => {
            let px = *font_size as f32;
            let width = text
                .lines()
                .map(|line| {
                    system_font().map_or(line.chars().count() as f32 * px * 0.8, |font| {
                        line.chars().map(|ch| font.metrics(ch, px).advance_width).sum()
                    })
                })
                .fold(0.0_f32, f32::max);
            RectI {
                x: position.0.floor() as i32 - 4,
                y: position.1.floor() as i32 - 4,
                width: width.ceil() as i32 + 12,
                height: text.lines().count() as i32 * (*font_size + 4) + 12,
            }
        }
        Annotation::Mosaic { start, end, .. } => RectI {
            x: start.0.min(end.0).floor() as i32 - 2,
            y: start.1.min(end.1).floor() as i32 - 2,
            width: (start.0.max(end.0).ceil() - start.0.min(end.0).floor()) as i32 + 5,
            height: (start.1.max(end.1).ceil() - start.1.min(end.1).floor()) as i32 + 5,
        },
    }
}

pub(crate) fn annotation_damage_regions(annotation: &Annotation, clip: RectI) -> Vec<RectI> {
    intersect_rect(annotation_bounds(annotation), clip).into_iter().collect()
}

// Selection UI is rendered only in the editor draft layer, never in the exported image.
const SELECT_BORDER_COLOR: [u8; 4] = [70, 207, 239, 245];
const SELECT_HANDLE_FILL: [u8; 4] = [244, 250, 255, 255];
const SELECT_OUTER_MARGIN: i32 = 6;
const ARROW_HANDLE_RADIUS: f64 = 9.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrowHandle {
    Start,
    End,
}

pub(crate) fn arrow_handle_hit_test(annotation: &Annotation, point: (f64, f64)) -> Option<ArrowHandle> {
    let Annotation::Arrow { start, end, .. } = annotation else {
        return None;
    };
    if point_distance(point, *start) <= ARROW_HANDLE_RADIUS {
        Some(ArrowHandle::Start)
    } else if point_distance(point, *end) <= ARROW_HANDLE_RADIUS {
        Some(ArrowHandle::End)
    } else {
        None
    }
}

pub(crate) fn selection_outer_bounds(annotation: &Annotation) -> RectI {
    let bounds = annotation_bounds(annotation);
    RectI {
        x: bounds.x - SELECT_OUTER_MARGIN,
        y: bounds.y - SELECT_OUTER_MARGIN,
        width: bounds.width + 2 * SELECT_OUTER_MARGIN,
        height: bounds.height + 2 * SELECT_OUTER_MARGIN,
    }
}

pub(crate) fn selected_annotation_hit_test(annotation: &Annotation, point: (f64, f64)) -> bool {
    if annotation_hit_test(annotation, point) || arrow_handle_hit_test(annotation, point).is_some() {
        return true;
    }
    let outline = selection_outer_bounds(annotation);
    if !outline.contains(point) {
        return false;
    }
    let x = point.0;
    let y = point.1;
    x <= f64::from(outline.x + 6)
        || x >= f64::from(outline.x + outline.width - 6)
        || y <= f64::from(outline.y + 6)
        || y >= f64::from(outline.y + outline.height - 6)
}

pub(crate) fn selected_annotation_damage(annotation: &Annotation, clip: RectI) -> Vec<RectI> {
    let bounds = selection_outer_bounds(annotation);
    if bounds.width <= 0 || bounds.height <= 0 {
        return Vec::new();
    }
    let mut regions = vec![
        RectI { x: bounds.x, y: bounds.y, width: bounds.width, height: 3 },
        RectI { x: bounds.x, y: bounds.y + bounds.height - 3, width: bounds.width, height: 3 },
        RectI { x: bounds.x, y: bounds.y, width: 3, height: bounds.height },
        RectI { x: bounds.x + bounds.width - 3, y: bounds.y, width: 3, height: bounds.height },
    ];
    if let Annotation::Arrow { start, end, .. } = annotation {
        for point in [start, end] {
            regions.push(RectI { x: point.0.round() as i32 - 9, y: point.1.round() as i32 - 9, width: 19, height: 19 });
        }
    }
    regions.into_iter().filter_map(|rect| intersect_rect(rect, clip)).collect()
}

fn draw_selected_annotation(raster: &mut Raster, annotation: &Annotation) {
    let bounds = selection_outer_bounds(annotation);
    if bounds.width <= 0 || bounds.height <= 0 {
        return;
    }
    // Dashed outline lies beyond the shape's existing paint/damage margins.
    for x in (bounds.x..bounds.x + bounds.width).step_by(12) {
        let length = (bounds.x + bounds.width - x).min(7);
        raster.fill_rect(RectI { x, y: bounds.y, width: length, height: 2 }, SELECT_BORDER_COLOR);
        raster.fill_rect(RectI { x, y: bounds.y + bounds.height - 2, width: length, height: 2 }, SELECT_BORDER_COLOR);
    }
    for y in (bounds.y..bounds.y + bounds.height).step_by(12) {
        let length = (bounds.y + bounds.height - y).min(7);
        raster.fill_rect(RectI { x: bounds.x, y, width: 2, height: length }, SELECT_BORDER_COLOR);
        raster.fill_rect(RectI { x: bounds.x + bounds.width - 2, y, width: 2, height: length }, SELECT_BORDER_COLOR);
    }
    if let Annotation::Arrow { start, end, .. } = annotation {
        for point in [start, end] {
            let (x, y) = transform_point(*point, 1.0, 1.0);
            raster.draw_disc(x, y, 7, SELECT_BORDER_COLOR);
            raster.draw_disc(x, y, 4, SELECT_HANDLE_FILL);
        }
    }
}

pub(crate) fn translate_annotation(annotation: &Annotation, dx: f64, dy: f64) -> Annotation {
    let move_point = |(x, y): (f64, f64)| (x + dx, y + dy);
    match annotation {
        Annotation::Pen { points, width, color } => {
            Annotation::Pen { points: points.iter().copied().map(move_point).collect(), width: *width, color: *color }
        }
        Annotation::Rectangle { start, end, width, color } => {
            Annotation::Rectangle { start: move_point(*start), end: move_point(*end), width: *width, color: *color }
        }
        Annotation::Arrow { start, end, width, color } => {
            Annotation::Arrow { start: move_point(*start), end: move_point(*end), width: *width, color: *color }
        }
        Annotation::Text { position, text, font_size, color } => Annotation::Text {
            position: move_point(*position),
            text: text.clone(),
            font_size: *font_size,
            color: *color,
        },
        Annotation::Mosaic { start, end, block_size } => {
            Annotation::Mosaic { start: move_point(*start), end: move_point(*end), block_size: *block_size }
        }
    }
}

pub(crate) fn translate_annotation_in_selection(
    annotation: &Annotation,
    dx: f64,
    dy: f64,
    selection: RectI,
) -> Annotation {
    let bounds = annotation_bounds(annotation);
    let min_dx = f64::from(selection.x - bounds.x);
    let max_dx = f64::from(selection.x + selection.width - bounds.x - bounds.width);
    let min_dy = f64::from(selection.y - bounds.y);
    let max_dy = f64::from(selection.y + selection.height - bounds.y - bounds.height);
    let dx = if min_dx <= max_dx { dx.clamp(min_dx, max_dx) } else { dx };
    let dy = if min_dy <= max_dy { dy.clamp(min_dy, max_dy) } else { dy };
    translate_annotation(annotation, dx, dy)
}

fn point_distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn point_segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let vx = b.0 - a.0;
    let vy = b.1 - a.1;
    let length_squared = vx * vx + vy * vy;
    if length_squared <= 1e-12 {
        return point_distance(p, a);
    }
    let t = (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / length_squared).clamp(0.0, 1.0);
    point_distance(p, (a.0 + vx * t, a.1 + vy * t))
}

pub(crate) fn key_to_ascii(key: u32, shift: bool) -> Option<char> {
    let pair = match key {
        2 => ('1', '!'),
        3 => ('2', '@'),
        4 => ('3', '#'),
        5 => ('4', '$'),
        6 => ('5', '%'),
        7 => ('6', '^'),
        8 => ('7', '&'),
        9 => ('8', '*'),
        10 => ('9', '('),
        11 => ('0', ')'),
        12 => ('-', '_'),
        13 => ('=', '+'),
        16 => ('q', 'Q'),
        17 => ('w', 'W'),
        18 => ('e', 'E'),
        19 => ('r', 'R'),
        20 => ('t', 'T'),
        21 => ('y', 'Y'),
        22 => ('u', 'U'),
        23 => ('i', 'I'),
        24 => ('o', 'O'),
        25 => ('p', 'P'),
        26 => ('[', '{'),
        27 => (']', '}'),
        30 => ('a', 'A'),
        31 => ('s', 'S'),
        32 => ('d', 'D'),
        33 => ('f', 'F'),
        34 => ('g', 'G'),
        35 => ('h', 'H'),
        36 => ('j', 'J'),
        37 => ('k', 'K'),
        38 => ('l', 'L'),
        39 => (';', ':'),
        40 => ('\'', '"'),
        41 => ('`', '~'),
        43 => ('\\', '|'),
        44 => ('z', 'Z'),
        45 => ('x', 'X'),
        46 => ('c', 'C'),
        47 => ('v', 'V'),
        48 => ('b', 'B'),
        49 => ('n', 'N'),
        50 => ('m', 'M'),
        51 => (',', '<'),
        52 => ('.', '>'),
        53 => ('/', '?'),
        57 => (' ', ' '),
        _ => return None,
    };
    Some(if shift { pair.1 } else { pair.0 })
}

fn draw_toolbar(raster: &mut Raster, toolbar: &ToolbarLayout, tool: EditorTool, selected_color: [u8; 4]) {
    raster.fill_rect(toolbar.bounds, TOOLBAR_BG);
    for (action, rect) in &toolbar.buttons {
        let active = matches!(action, ToolbarAction::Tool(active_tool) if *active_tool == tool);
        raster.fill_rect(*rect, if active { TOOLBAR_ACTIVE_BG } else { TOOLBAR_BUTTON_BG });
        if *action == ToolbarAction::Color {
            raster.fill_rect(RectI { x: rect.x + 9, y: rect.y + 9, width: 22, height: 22 }, selected_color);
        } else {
            let (svg, color) = toolbar_icon(*action);
            draw_svg_icon(raster, *rect, svg, color);
        }
    }
}

fn toolbar_icon(action: ToolbarAction) -> (&'static str, [u8; 4]) {
    match action {
        ToolbarAction::Tool(EditorTool::Pen) => (PEN_ICON, TOOLBAR_ICON),
        ToolbarAction::Tool(EditorTool::Rectangle) => (RECTANGLE_ICON, TOOLBAR_ICON),
        ToolbarAction::Tool(EditorTool::Arrow) => (ARROW_ICON, TOOLBAR_ICON),
        ToolbarAction::Tool(EditorTool::Text) => (TEXT_ICON, TOOLBAR_ICON),
        ToolbarAction::Tool(EditorTool::Mosaic) => (MOSAIC_ICON, TOOLBAR_ICON),
        ToolbarAction::Color => (PEN_ICON, TOOLBAR_ICON),
        ToolbarAction::Undo => (UNDO_ICON, TOOLBAR_ICON),
        ToolbarAction::Redo => (REDO_ICON, TOOLBAR_ICON),
        ToolbarAction::Confirm => (CONFIRM_ICON, TOOLBAR_CONFIRM),
        ToolbarAction::Cancel => (CANCEL_ICON, TOOLBAR_CANCEL),
    }
}

fn draw_svg_icon(raster: &mut Raster, bounds: RectI, svg: &str, color: [u8; 4]) {
    let icon_size = (bounds.width.min(bounds.height) - 14).max(1);
    let origin_x = bounds.x + (bounds.width - icon_size) / 2;
    let origin_y = bounds.y + (bounds.height - icon_size) / 2;
    let scale = f64::from(icon_size) / ICON_VIEWBOX;
    let map = |x: f64, y: f64| (origin_x + (x * scale).round() as i32, origin_y + (y * scale).round() as i32);

    visit_svg_tags(svg, "<line", |tag| {
        let Some(x1) = svg_number(tag, "x1") else { return };
        let Some(y1) = svg_number(tag, "y1") else { return };
        let Some(x2) = svg_number(tag, "x2") else { return };
        let Some(y2) = svg_number(tag, "y2") else { return };
        let width = svg_number(tag, "stroke-width").unwrap_or(2.0);
        raster.draw_line(map(x1, y1), map(x2, y2), (width * scale).round().max(1.0) as i32, color);
    });

    visit_svg_tags(svg, "<rect", |tag| {
        let Some(x) = svg_number(tag, "x") else { return };
        let Some(y) = svg_number(tag, "y") else { return };
        let Some(width) = svg_number(tag, "width") else { return };
        let Some(height) = svg_number(tag, "height") else { return };
        let stroke = svg_number(tag, "stroke-width").unwrap_or(2.0);
        let top_left = map(x, y);
        let bottom_right = map(x + width, y + height);
        raster.draw_rect_outline(
            RectI {
                x: top_left.0,
                y: top_left.1,
                width: bottom_right.0 - top_left.0,
                height: bottom_right.1 - top_left.1,
            },
            (stroke * scale).round().max(1.0) as i32,
            color,
        );
    });

    visit_svg_tags(svg, "<polyline", |tag| {
        let Some(points) = svg_attr(tag, "points") else { return };
        let stroke = svg_number(tag, "stroke-width").unwrap_or(2.0);
        let width = (stroke * scale).round().max(1.0) as i32;
        let points = points
            .split_ascii_whitespace()
            .filter_map(|point| {
                let (x, y) = point.split_once(',')?;
                Some(map(x.parse().ok()?, y.parse().ok()?))
            })
            .collect::<Vec<_>>();
        for pair in points.windows(2) {
            raster.draw_line(pair[0], pair[1], width, color);
        }
    });
}

fn visit_svg_tags(mut svg: &str, prefix: &str, mut visitor: impl FnMut(&str)) {
    while let Some(start) = svg.find(prefix) {
        let tail = &svg[start..];
        let Some(end) = tail.find('>') else {
            return;
        };
        visitor(&tail[..=end]);
        svg = &tail[end + 1..];
    }
}

fn svg_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let rest = &tag[start..];
    Some(&rest[..rest.find('"')?])
}

fn svg_number(tag: &str, name: &str) -> Option<f64> {
    svg_attr(tag, name)?.parse().ok()
}

fn draw_annotation(raster: &mut Raster, annotation: &Annotation, sx: f64, sy: f64, source: Option<&MosaicSource>) {
    match annotation {
        Annotation::Pen { points, width, color } => draw_pen(raster, points, *width, sx, sy, *color),
        Annotation::Rectangle { start, end, width, color } => {
            let start = transform_point(*start, sx, sy);
            let end = transform_point(*end, sx, sy);
            if let Some(rect) = rect_from_points(start, end) {
                raster.draw_rect_outline(rect, scaled_width(sx, sy, f64::from(*width)), *color);
            }
        }
        Annotation::Arrow { start, end, width, color } => {
            draw_arrow(
                raster,
                transform_point(*start, sx, sy),
                transform_point(*end, sx, sy),
                scaled_width(sx, sy, f64::from(*width)),
                *color,
            );
        }
        Annotation::Text { position, text, font_size, color } => {
            draw_text(raster, *position, text, *font_size, *color, sx, sy);
        }
        Annotation::Mosaic { start, end, block_size } => {
            draw_mosaic(raster, *start, *end, *block_size, sx, sy, source);
        }
    }
}

fn draw_draft(raster: &mut Raster, draft: &DraftAnnotation, sx: f64, sy: f64, source: Option<&MosaicSource>) {
    match draft {
        DraftAnnotation::Pen { points, width, color } => draw_pen(raster, points, *width, sx, sy, *color),
        DraftAnnotation::Rectangle { start, end, width, color } => {
            if let Some(rect) = rect_from_points(transform_point(*start, sx, sy), transform_point(*end, sx, sy)) {
                raster.draw_rect_outline(rect, scaled_width(sx, sy, f64::from(*width)), *color);
            }
        }
        DraftAnnotation::Arrow { start, end, width, color } => {
            draw_arrow(
                raster,
                transform_point(*start, sx, sy),
                transform_point(*end, sx, sy),
                scaled_width(sx, sy, f64::from(*width)),
                *color,
            );
        }
        DraftAnnotation::Mosaic { start, end, .. } => {
            // Only the outline changes while dragging; apply the full pixel mosaic on release.
            if let Some(rect) = rect_from_points(transform_point(*start, sx, sy), transform_point(*end, sx, sy)) {
                raster.draw_rect_outline(rect, 2, [220, 231, 248, 255]);
                let _ = source;
            }
        }
    }
}

fn draw_mosaic(
    raster: &mut Raster,
    start: (f64, f64),
    end: (f64, f64),
    block_size: i32,
    sx: f64,
    sy: f64,
    source: Option<&MosaicSource>,
) {
    let Some(rect) = rect_from_points(transform_point(start, sx, sy), transform_point(end, sx, sy)) else {
        return;
    };
    let block_x = (f64::from(block_size) * sx).round().max(2.0) as i32;
    let block_y = (f64::from(block_size) * sy).round().max(2.0) as i32;
    let clipped = raster.clip.and_then(|clip| intersect_rect(rect, clip)).unwrap_or(rect);
    let max_x = rect.x.saturating_add(rect.width);
    let max_y = rect.y.saturating_add(rect.height);
    let start_col = (clipped.x - rect.x).max(0) / block_x;
    let start_row = (clipped.y - rect.y).max(0) / block_y;
    let end_col = ((clipped.x + clipped.width - rect.x + block_x - 1) / block_x).max(start_col);
    let end_row = ((clipped.y + clipped.height - rect.y + block_y - 1) / block_y).max(start_row);
    for row in start_row..end_row {
        for col in start_col..end_col {
            let px = rect.x + col * block_x;
            let py = rect.y + row * block_y;
            let width = block_x.min(max_x - px);
            let height = block_y.min(max_y - py);
            if width <= 0 || height <= 0 {
                continue;
            }
            let color = if let Some(source) = source {
                mosaic_block_color(source, px, py, width, height, sx, sy)
            } else if (row + col) % 2 == 0 {
                [104, 120, 141, 255]
            } else {
                [160, 174, 194, 255]
            };
            raster.fill_rect(RectI { x: px, y: py, width, height }, color);
        }
    }
}

fn mosaic_block_color(source: &MosaicSource, x: i32, y: i32, width: i32, height: i32, sx: f64, sy: f64) -> [u8; 4] {
    let image_size = source.image.size();
    let logical_x = f64::from(x) / sx;
    let logical_y = f64::from(y) / sy;
    let logical_w = f64::from(width) / sx;
    let logical_h = f64::from(height) / sy;
    let scale_x = f64::from(image_size.width) / f64::from(source.logical_size.width);
    let scale_y = f64::from(image_size.height) / f64::from(source.logical_size.height);
    let bytes = source.image.pixels();
    let mut sum = [0_u64; 3];
    for iy in 0..4 {
        for ix in 0..4 {
            let fx = ((logical_x + logical_w * (f64::from(ix) + 0.5) / 4.0) * scale_x)
                .floor()
                .clamp(0.0, f64::from(image_size.width - 1)) as usize;
            let fy = ((logical_y + logical_h * (f64::from(iy) + 0.5) / 4.0) * scale_y)
                .floor()
                .clamp(0.0, f64::from(image_size.height - 1)) as usize;
            let offset = (fy * image_size.width as usize + fx) * 4;
            for i in 0..3 {
                sum[i] += u64::from(bytes[offset + i]);
            }
        }
    }
    [(sum[0] / 16) as u8, (sum[1] / 16) as u8, (sum[2] / 16) as u8, 255]
}

fn draw_pen(raster: &mut Raster, points: &[(f64, f64)], stroke_width: i32, sx: f64, sy: f64, color: [u8; 4]) {
    let width = scaled_width(sx, sy, f64::from(stroke_width));
    for pair in points.windows(2) {
        raster.draw_line(transform_point(pair[0], sx, sy), transform_point(pair[1], sx, sy), width, color);
    }
    if let Some(point) = points.first() {
        let point = transform_point(*point, sx, sy);
        raster.draw_disc(point.0, point.1, width.max(1), color);
    }
}

fn draw_arrow(raster: &mut Raster, start: (i32, i32), end: (i32, i32), width: i32, color: [u8; 4]) {
    raster.draw_line(start, end, width, color);
    let dx = f64::from(end.0 - start.0);
    let dy = f64::from(end.1 - start.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 {
        return;
    }
    let ux = dx / length;
    let uy = dy / length;
    let head = 14.0_f64.max(f64::from(width) * 4.0);
    let side = head * 0.55;
    let base = (f64::from(end.0) - ux * head, f64::from(end.1) - uy * head);
    let left = ((base.0 + -uy * side).round() as i32, (base.1 + ux * side).round() as i32);
    let right = ((base.0 - -uy * side).round() as i32, (base.1 - ux * side).round() as i32);
    raster.draw_line(end, left, width, color);
    raster.draw_line(end, right, width, color);
}

fn draw_text(raster: &mut Raster, position: (f64, f64), text: &str, font_size: i32, color: [u8; 4], sx: f64, sy: f64) {
    let position = transform_point(position, sx, sy);
    let px = (f64::from(font_size) * (sx + sy) * 0.5).max(1.0) as f32;
    if let Some(font) = system_font() {
        raster.draw_text_font(position, text, px, color, font);
    } else {
        raster.draw_text_bitmap(position, text, ((px / 8.0).round() as i32).max(1), color);
    }
}

fn draw_text_selection(raster: &mut Raster, position: (f64, f64), text: &str, font_size: i32, sx: f64, sy: f64) {
    let position = transform_point(position, sx, sy);
    let px = (f64::from(font_size) * (sx + sy) * 0.5).max(1.0) as f32;
    let scale = ((px / 8.0).round() as i32).max(1);
    let line_height = px.ceil() as i32 + 4;
    let font = system_font();
    for (index, line) in text.split('\n').enumerate() {
        let width = if let Some(font) = font {
            line.chars().map(|ch| font.metrics(ch, px).advance_width).sum::<f32>().ceil() as i32
        } else {
            i32::try_from(line.chars().count()).unwrap_or(i32::MAX).saturating_mul(6 * scale)
        };
        if width > 0 {
            raster.fill_rect(
                RectI {
                    x: position.0,
                    y: position.1 + i32::try_from(index).unwrap_or(i32::MAX).saturating_mul(line_height),
                    width,
                    height: line_height,
                },
                TEXT_SELECTION_COLOR,
            );
        }
    }
}

fn draw_text_cursor(
    raster: &mut Raster,
    position: (f64, f64),
    text: &str,
    font_size: i32,
    color: [u8; 4],
    sx: f64,
    sy: f64,
) {
    let position = transform_point(position, sx, sy);
    let px = (f64::from(font_size) * (sx + sy) * 0.5).max(1.0) as f32;
    let scale = ((px / 8.0).round() as i32).max(1);
    let cursor_x = if let Some(font) = system_font() {
        let line = text.rsplit('\n').next().unwrap_or_default();
        let advance = line.chars().map(|ch| font.metrics(ch, px).advance_width).sum::<f32>();
        position.0 + advance.round() as i32
    } else {
        let line_len = text.rsplit('\n').next().unwrap_or_default().chars().count();
        position.0 + i32::try_from(line_len).unwrap_or(i32::MAX).saturating_mul(6 * scale)
    };
    let line_count = text.chars().filter(|ch| *ch == '\n').count();
    let cursor_y = position.1 + i32::try_from(line_count).unwrap_or(i32::MAX).saturating_mul(px.ceil() as i32 + 4);
    raster.fill_rect(RectI { x: cursor_x + 1, y: cursor_y, width: 2, height: (px.ceil() as i32).max(16) }, color);
}

pub(crate) fn preload_system_font() {
    ensure_system_font_loading();
}

fn system_font() -> Option<&'static Font> {
    ensure_system_font_loading();
    SYSTEM_FONT.get()
}

fn ensure_system_font_loading() {
    if SYSTEM_FONT.get().is_some()
        || SYSTEM_FONT_LOADING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err()
    {
        return;
    }
    let _ = thread::Builder::new().name("lisnap-font-loader".into()).spawn(|| {
        if let Some(font) = load_system_font() {
            let _ = SYSTEM_FONT.set(font);
        }
    });
}

fn load_system_font() -> Option<Font> {
    let configured = std::env::var_os("LISNAP_FONT").map(PathBuf::from).filter(|path| path.is_file());
    configured
        .into_iter()
        .chain(fontconfig_cjk_fonts())
        .find_map(|path| fs::read(path).ok().and_then(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok()))
}

fn fontconfig_cjk_fonts() -> Vec<PathBuf> {
    let Ok(output) = Command::new("fc-match").args(["-s", "-f", "%{file}\n", ":lang=zh-cn"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for path in stdout.lines().map(str::trim).filter(|line| !line.is_empty()).map(PathBuf::from) {
        if path.is_file() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn scaled_width(sx: f64, sy: f64, logical: f64) -> i32 {
    (logical * (sx + sy) * 0.5).round().max(1.0) as i32
}

fn transform_point(point: (f64, f64), sx: f64, sy: f64) -> (i32, i32) {
    ((point.0 * sx).round() as i32, (point.1 * sy).round() as i32)
}

fn rect_from_points(start: (i32, i32), end: (i32, i32)) -> Option<RectI> {
    let x1 = start.0.min(end.0);
    let y1 = start.1.min(end.1);
    let x2 = start.0.max(end.0);
    let y2 = start.1.max(end.1);
    (x2 > x1 && y2 > y1).then_some(RectI { x: x1, y: y1, width: x2 - x1, height: y2 - y1 })
}

struct Raster {
    size: PixelSize,
    origin: (i32, i32),
    pixels: Vec<u8>,
    clip: Option<RectI>,
}

impl Raster {
    fn blit_rgba(&mut self, rect: RectI, pixels: &[u8]) {
        let width = rect.width.max(0) as usize;
        let height = rect.height.max(0) as usize;
        if width.checked_mul(height).and_then(|size| size.checked_mul(4)) != Some(pixels.len()) {
            return;
        }
        let backing = RectI {
            x: self.origin.0,
            y: self.origin.1,
            width: self.size.width as i32,
            height: self.size.height as i32,
        };
        let Some(region) = intersect_rect(rect, backing).and_then(|r| {
            self.clip.and_then(|clip| intersect_rect(r, clip)).or_else(|| self.clip.is_none().then_some(r))
        }) else {
            return;
        };
        let row_bytes = region.width as usize * 4;
        for y in region.y..region.y + region.height {
            let source_offset = ((y - rect.y) as usize * width + (region.x - rect.x) as usize) * 4;
            let target_offset =
                ((y - self.origin.1) as usize * self.size.width as usize + (region.x - self.origin.0) as usize) * 4;
            self.pixels[target_offset..target_offset + row_bytes]
                .copy_from_slice(&pixels[source_offset..source_offset + row_bytes]);
        }
    }

    fn fill_rect_replace(&mut self, rect: RectI, color: [u8; 4]) {
        let bounds = RectI {
            x: self.origin.0,
            y: self.origin.1,
            width: self.size.width as i32,
            height: self.size.height as i32,
        };
        let Some(region) = intersect_rect(rect, bounds).and_then(|r| {
            self.clip.and_then(|clip| intersect_rect(r, clip)).or_else(|| self.clip.is_none().then_some(r))
        }) else {
            return;
        };
        for y in region.y..region.y + region.height {
            let target_offset =
                ((y - self.origin.1) as usize * self.size.width as usize + (region.x - self.origin.0) as usize) * 4;
            let end = target_offset + region.width as usize * 4;
            for pixel in self.pixels[target_offset..end].chunks_exact_mut(4) {
                pixel.copy_from_slice(&color);
            }
        }
    }

    fn blend_pixel(&mut self, x: i32, y: i32, color: [u8; 4]) {
        if self.clip.is_some_and(|clip| !clip.contains((f64::from(x), f64::from(y)))) {
            return;
        }
        let local_x = x - self.origin.0;
        let local_y = y - self.origin.1;
        if local_x < 0 || local_y < 0 || local_x >= self.size.width as i32 || local_y >= self.size.height as i32 {
            return;
        }
        let index = (local_y as usize * self.size.width as usize + local_x as usize) * 4;
        let sa = u32::from(color[3]);
        if sa == 255 {
            self.pixels[index..index + 4].copy_from_slice(&color);
            return;
        }
        let da = u32::from(self.pixels[index + 3]);
        let inv = 255 - sa;
        let out_a = sa + (da * inv + 127) / 255;
        if out_a == 0 {
            return;
        }
        for (channel, source) in color.iter().take(3).enumerate() {
            let src = u32::from(*source);
            let dst = u32::from(self.pixels[index + channel]);
            let numerator = src * sa * 255 + dst * da * inv;
            self.pixels[index + channel] = ((numerator + out_a * 127) / (out_a * 255)).min(255) as u8;
        }
        self.pixels[index + 3] = out_a.min(255) as u8;
    }

    fn fill_rect(&mut self, rect: RectI, color: [u8; 4]) {
        let x1 = rect.x.max(self.origin.0);
        let y1 = rect.y.max(self.origin.1);
        let x2 = (rect.x + rect.width).min(self.origin.0 + self.size.width as i32);
        let y2 = (rect.y + rect.height).min(self.origin.1 + self.size.height as i32);
        for y in y1..y2 {
            for x in x1..x2 {
                self.blend_pixel(x, y, color);
            }
        }
    }

    fn draw_disc(&mut self, cx: i32, cy: i32, radius: i32, color: [u8; 4]) {
        let radius = radius.max(1);
        let rr = radius * radius;
        for y in (cy - radius)..=(cy + radius) {
            for x in (cx - radius)..=(cx + radius) {
                let dx = x - cx;
                let dy = y - cy;
                if dx * dx + dy * dy <= rr {
                    self.blend_pixel(x, y, color);
                }
            }
        }
    }

    fn draw_line(&mut self, start: (i32, i32), end: (i32, i32), width: i32, color: [u8; 4]) {
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        let steps = dx.abs().max(dy.abs()).max(1);
        let radius = (width.max(1) + 1) / 2;
        for step in 0..=steps {
            let t = f64::from(step) / f64::from(steps);
            let x = (f64::from(start.0) + f64::from(dx) * t).round() as i32;
            let y = (f64::from(start.1) + f64::from(dy) * t).round() as i32;
            self.draw_disc(x, y, radius, color);
        }
    }

    fn draw_rect_outline(&mut self, rect: RectI, width: i32, color: [u8; 4]) {
        let right = rect.x + rect.width;
        let bottom = rect.y + rect.height;
        self.draw_line((rect.x, rect.y), (right, rect.y), width, color);
        self.draw_line((right, rect.y), (right, bottom), width, color);
        self.draw_line((right, bottom), (rect.x, bottom), width, color);
        self.draw_line((rect.x, bottom), (rect.x, rect.y), width, color);
    }

    fn draw_text_font(&mut self, position: (i32, i32), text: &str, px: f32, color: [u8; 4], font: &Font) {
        let mut cursor_x = position.0 as f32;
        let mut cursor_y = position.1;
        let line_height = px.ceil() as i32 + 4;
        for ch in text.chars() {
            if ch == '\n' {
                cursor_x = position.0 as f32;
                cursor_y += line_height;
                continue;
            }
            let (metrics, bitmap) = font.rasterize(ch, px);
            let glyph_x = cursor_x.round() as i32 + metrics.xmin;
            let glyph_y = cursor_y + (px.ceil() as i32 - metrics.height as i32 - metrics.ymin);
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let coverage = bitmap[row * metrics.width + col];
                    if coverage == 0 {
                        continue;
                    }
                    let alpha = (u16::from(color[3]) * u16::from(coverage) / 255) as u8;
                    self.blend_pixel(glyph_x + col as i32, glyph_y + row as i32, [color[0], color[1], color[2], alpha]);
                }
            }
            cursor_x += metrics.advance_width.max(px * 0.35);
        }
    }

    fn draw_text_bitmap(&mut self, position: (i32, i32), text: &str, scale: i32, color: [u8; 4]) {
        let scale = scale.max(1);
        let mut cursor_x = position.0;
        let mut cursor_y = position.1;
        for ch in text.chars() {
            if ch == '\n' {
                cursor_x = position.0;
                cursor_y += 9 * scale;
                continue;
            }
            let glyph = glyph5x7(ch);
            for (row, bits) in glyph.into_iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.fill_rect(
                            RectI {
                                x: cursor_x + col * scale,
                                y: cursor_y + i32::try_from(row).unwrap_or(0) * scale,
                                width: scale,
                                height: scale,
                            },
                            color,
                        );
                    }
                }
            }
            cursor_x += 6 * scale;
        }
    }
}

fn rgba_to_wayland_argb_in_place(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let [r, g, b, a] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        let alpha = u16::from(a);
        let premul = |value: u8| ((u16::from(value) * alpha + 127) / 255) as u8;
        pixel.copy_from_slice(&[premul(b), premul(g), premul(r), a]);
    }
}

fn glyph5x7(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10011, 0b10101, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        '6' => [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        ' ' => [0; 7],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        ',' => [0, 0, 0, 0, 0b00110, 0b00110, 0b00100],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        ':' => [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '\\' => [0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '*' => [0, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0],
        '#' => [0b01010, 0b11111, 0b01010, 0b01010, 0b11111, 0b01010, 0],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        _ => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lisnap_core::ScreenInfo;
    use std::time::SystemTime;

    #[test]
    fn toolbar_stays_on_screen_and_hits_buttons() {
        let layout =
            toolbar_layout(RectI { x: 1800, y: 900, width: 100, height: 100 }, PixelSize { width: 1920, height: 1080 });
        assert!(layout.bounds.x >= 0);
        assert!(layout.bounds.y >= 0);
        assert!(layout.bounds.x + layout.bounds.width <= 1920);
        assert!(layout.bounds.y + layout.bounds.height <= 1080);
        let (_, first) = layout.buttons[1];
        assert_eq!(
            layout.hit_test((f64::from(first.x + 2), f64::from(first.y + 2))),
            Some(ToolbarAction::Tool(EditorTool::Pen))
        );
    }

    #[test]
    fn toolbar_icons_render_from_svg_resources() {
        let actions = [
            ToolbarAction::Color,
            ToolbarAction::Tool(EditorTool::Pen),
            ToolbarAction::Tool(EditorTool::Rectangle),
            ToolbarAction::Tool(EditorTool::Arrow),
            ToolbarAction::Tool(EditorTool::Text),
            ToolbarAction::Tool(EditorTool::Mosaic),
            ToolbarAction::Undo,
            ToolbarAction::Redo,
            ToolbarAction::Confirm,
            ToolbarAction::Cancel,
        ];
        for action in actions {
            let mut raster = Raster {
                size: PixelSize { width: 40, height: 40 },
                origin: (0, 0),
                pixels: vec![0; 40 * 40 * 4],
                clip: None,
            };
            let (svg, color) = toolbar_icon(action);
            assert!(svg.starts_with("<svg"));
            draw_svg_icon(&mut raster, RectI { x: 0, y: 0, width: 40, height: 40 }, svg, color);
            assert!(
                raster.pixels.chunks_exact(4).any(|pixel| pixel[3] != 0),
                "SVG icon for {action:?} rendered no pixels"
            );
        }
    }

    #[test]
    fn selection_dashes_lie_outside_shapes_and_are_preview_only() {
        let annotation =
            Annotation::Rectangle { start: (60.0, 60.0), end: (140.0, 130.0), width: 3, color: PALETTE[6].1 };
        let region = RectI { x: 35, y: 35, width: 130, height: 120 };
        let bounds = annotation_bounds(&annotation);
        let frame = selection_outer_bounds(&annotation);
        assert!(frame.x < bounds.x && frame.y < bounds.y);
        assert!(frame.x + frame.width > bounds.x + bounds.width);
        assert!(selected_annotation_hit_test(&annotation, (f64::from(frame.x + 1), f64::from(frame.y + 8))));
        assert!(selected_annotation_hit_test(&annotation, (100.0, 100.0)));
        assert!(!selected_annotation_hit_test(&annotation, (10.0, 10.0)));

        let original = render_annotations_patch(region, std::slice::from_ref(&annotation));
        let selected = render_draft_patch_with_move(region, None, None, false, None, Some(&annotation), None);
        let sample = |image: &[u8], x: i32, y: i32| {
            image[((y - region.y) as usize * region.width as usize + (x - region.x) as usize) * 4 + 3]
        };
        assert_eq!(sample(&original, frame.x, frame.y), 0);
        assert_ne!(sample(&selected, frame.x, frame.y), 0);
        assert!(
            render_draft_patch_with_move(region, None, None, false, None, None, None).iter().all(|byte| *byte == 0)
        );
    }

    #[test]
    fn selection_damage_is_thin_and_arrow_endpoint_handles_are_interactive() {
        let annotation =
            Annotation::Rectangle { start: (70.0, 80.0), end: (1670.0, 880.0), width: 3, color: DEFAULT_COLOR };
        let clip = RectI { x: 0, y: 0, width: 1920, height: 1080 };
        let regions = selected_annotation_damage(&annotation, clip);
        assert_eq!(regions.len(), 4);
        let pixel_count: i64 = regions.iter().map(|rect| i64::from(rect.width) * i64::from(rect.height)).sum();
        assert!(pixel_count < 20_000);
        let clipped = RectI { x: 0, y: 0, width: 120, height: 120 };
        assert!(
            selected_annotation_damage(&annotation, clipped).iter().all(|rect| {
                rect.x >= 0 && rect.y >= 0 && rect.x + rect.width <= 120 && rect.y + rect.height <= 120
            })
        );

        let arrow = Annotation::Arrow { start: (120.0, 130.0), end: (280.0, 240.0), width: 4, color: DEFAULT_COLOR };
        assert_eq!(arrow_handle_hit_test(&arrow, (125.0, 130.0)), Some(ArrowHandle::Start));
        assert_eq!(arrow_handle_hit_test(&arrow, (280.0, 245.0)), Some(ArrowHandle::End));
        assert_eq!(arrow_handle_hit_test(&arrow, (200.0, 185.0)), None);
        assert!(selected_annotation_hit_test(&arrow, (120.0, 130.0)));
        assert_eq!(selected_annotation_damage(&arrow, clip).len(), 6);
        let region = RectI { x: 100, y: 110, width: 200, height: 150 };
        let selected = render_draft_patch_with_move(region, None, None, false, None, Some(&arrow), None);
        assert!(selected.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }

    #[test]
    fn rectangle_draft_damage_only_tracks_outline_strips() {
        let selection = RectI { x: 0, y: 0, width: 1920, height: 1080 };
        let draft =
            DraftAnnotation::Rectangle { start: (100.0, 100.0), end: (1700.0, 900.0), width: 3, color: DEFAULT_COLOR };
        let regions = draft_damage_regions(Some(&draft), None, selection);
        assert_eq!(regions.len(), 4);
        let damaged_pixels: i64 = regions.iter().map(|rect| i64::from(rect.width) * i64::from(rect.height)).sum();
        assert!(damaged_pixels < i64::from(selection.width) * i64::from(selection.height) / 20);
    }

    #[test]
    fn patch_renderer_uses_region_local_storage() {
        let draft =
            DraftAnnotation::Rectangle { start: (100.0, 100.0), end: (500.0, 300.0), width: 3, color: DEFAULT_COLOR };
        let region = RectI { x: 96, y: 96, width: 409, height: 9 };
        let pixels = render_draft_patch(region, Some(&draft), None, false);
        assert_eq!(pixels.len(), region.width as usize * region.height as usize * 4);
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }

    #[test]
    fn empty_text_draft_renders_visible_caret() {
        let draft = TextDraft::new((20.0, 20.0));
        let region = text_damage_rect(&draft);
        let pixels = render_draft_patch(region, None, Some(&draft), true);
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }

    #[test]
    fn hidden_empty_text_cursor_renders_transparent_patch() {
        let draft = TextDraft::new((20.0, 20.0));
        let region = text_damage_rect(&draft);
        let pixels = render_draft_patch(region, None, Some(&draft), false);
        assert!(pixels.chunks_exact(4).all(|pixel| pixel[3] == 0));
    }

    #[test]
    fn chinese_preedit_damage_reserves_full_glyph_width() {
        let mut draft = TextDraft::new((100.0, 80.0));
        draft.text = "中文".into();
        draft.preedit = "输入".into();
        let rect = text_damage_rect(&draft);
        assert!(rect.width >= 5 * 30);
        assert!(rect.height >= 32);
    }

    #[test]
    fn text_select_all_replaces_utf8_and_undo_redo_round_trips() {
        let mut text = TextDraft::new((12.0, 18.0));
        assert!(!text.select_all());
        assert!(text.insert_text("中文abc"));
        assert!(text.select_all());
        assert!(text.selected_all);
        assert!(text.insert_text("替换"));
        assert_eq!(text.text, "替换");
        assert!(!text.selected_all);
        assert!(text.undo());
        assert_eq!(text.text, "中文abc");
        assert!(text.redo());
        assert_eq!(text.text, "替换");
        assert!(text.backspace());
        assert_eq!(text.text, "替");
        assert!(text.undo());
        assert_eq!(text.text, "替换");
        assert!(text.select_all());
        assert!(text.delete_selected());
        assert!(text.text.is_empty());
        assert!(text.undo());
        assert_eq!(text.text, "替换");
    }

    #[test]
    fn text_undo_clears_redo_on_new_edit_and_preserves_ime_preedit() {
        let mut text = TextDraft::new((0.0, 0.0));
        assert!(text.insert_text("A"));
        assert!(text.insert_text("B"));
        assert!(text.undo());
        assert_eq!(text.text, "A");
        assert!(text.insert_text("中"));
        assert!(!text.redo());
        text.preedit = "zhong".into();
        assert!(!text.select_all());
        assert_eq!(text.text, "A中");
    }

    #[test]
    fn selected_text_renders_visible_highlight() {
        let mut text = TextDraft::new((10.0, 10.0));
        text.insert_text("Hello中文");
        let region = text_damage_rect(&text);
        let before = render_draft_patch(region, None, Some(&text), false);
        text.select_all();
        let after = render_draft_patch(region, None, Some(&text), false);
        assert_ne!(before, after);
        assert!(after.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn context_menus_clamp_to_screen_and_hit_test_sizes() {
        let screen = PixelSize { width: 800, height: 600 };
        let stroke = context_menu_layout(EditorTool::Pen, (790.0, 580.0), screen);
        assert_eq!(stroke.kind, ContextMenuKind::Stroke);
        assert!(stroke.bounds.x + stroke.bounds.width <= 800);
        assert!(stroke.bounds.y + stroke.bounds.height <= 600);
        assert_eq!(stroke.options.iter().map(|(value, _)| *value).collect::<Vec<_>>(), [1, 2, 3, 5, 8, 12]);
        let (value, rect) = stroke.options[4];
        assert_eq!(stroke.hit_test((f64::from(rect.x + 5), f64::from(rect.y + 5))), Some(value));
        assert_eq!(stroke.hit_test((0.0, 0.0)), None);

        let font = context_menu_layout(EditorTool::Text, (20.0, 20.0), screen);
        assert_eq!(font.kind, ContextMenuKind::Font);
        assert_eq!(font.options.iter().map(|(value, _)| *value).collect::<Vec<_>>(), [16, 20, 24, 32, 40, 48]);
        assert_ne!(
            render_context_menu_patch(&font, 24, DEFAULT_COLOR, None),
            render_context_menu_patch(&font, 48, DEFAULT_COLOR, None)
        );
    }

    #[test]
    fn palette_and_picker_share_position_near_screen_edges() {
        let screen = PixelSize { width: 1920, height: 1080 };
        for point in [(1900.0, 1050.0), (900.0, 950.0), (4.0, 2.0), (1900.0, 2.0)] {
            let palette = color_menu_layout(point, screen);
            let custom = custom_color_menu_layout((f64::from(palette.bounds.x), f64::from(palette.bounds.y)), screen);
            assert_eq!((palette.bounds.x, palette.bounds.y), (custom.bounds.x, custom.bounds.y));
            assert_eq!(palette.bounds.width, STANDARD_MENU_WIDTH);
            assert_eq!(palette.bounds.height, STANDARD_MENU_HEIGHT);
            let (custom_id, custom_row) = palette.options.last().unwrap();
            assert_eq!(*custom_id, PALETTE.len() as i32);
            assert!(palette.bounds.contains((f64::from(custom_row.x + 5), f64::from(custom_row.y + 5))));
            assert!(custom.bounds.x >= 0 && custom.bounds.y >= 0);
            assert!(custom.bounds.x + custom.bounds.width <= screen.width as i32);
            assert!(custom.bounds.y + custom.bounds.height <= screen.height as i32);
        }
    }

    #[test]
    fn hsv_gradient_is_cached_until_hue_changes_and_pointer_hits_exact_pixel() {
        let mut editor = CustomColorEditor::new([255, 76, 76, 255]);
        let original = Arc::clone(&editor.sv_gradient);
        assert_eq!(original.len(), SV_WIDTH * SV_HEIGHT * 4);
        assert!(editor.set_saturation_value(0.2, 0.7));
        assert!(Arc::ptr_eq(&original, &editor.sv_gradient));
        assert!(editor.set_saturation_value(0.85, 0.5));
        assert!(Arc::ptr_eq(&original, &editor.sv_gradient));
        assert!(editor.set_hue(205.0));
        assert!(!Arc::ptr_eq(&original, &editor.sv_gradient));
        let changed = Arc::clone(&editor.sv_gradient);
        assert!(!editor.set_hue(205.0));
        assert!(Arc::ptr_eq(&changed, &editor.sv_gradient));

        let screen = PixelSize { width: 1024, height: 768 };
        let menu = custom_color_menu_layout((100.0, 80.0), screen);
        let square = menu.options.iter().find(|(id, _)| *id == CUSTOM_SV_ID).unwrap().1;
        let point = (f64::from(square.x + 53), f64::from(square.y + 42));
        let (sat, value) = custom_sat_value_at(&menu, point).unwrap();
        assert!(editor.set_saturation_value(sat, value));
        let pixels = render_context_menu_patch(&menu, PALETTE.len() as i32, editor.color, Some(&editor));
        let pixel_offset = ((point.1 as i32 - menu.bounds.y) as usize * CONTEXT_MENU_SIZE.width as usize
            + (point.0 as i32 - menu.bounds.x) as usize)
            * 4;
        let pixel =
            [pixels[pixel_offset + 2], pixels[pixel_offset + 1], pixels[pixel_offset], pixels[pixel_offset + 3]];
        // The cursor marker covers this pixel. Check the adjacent unchanged pixel
        // against the same hue/saturation/value grid instead.
        assert_eq!(pixel[3], 255);
        let far_point = (f64::from(square.x + 160), f64::from(square.y + 88));
        let (far_s, far_v) = custom_sat_value_at(&menu, far_point).unwrap();
        let expected = hsv_to_rgb(editor.hue, far_s, far_v);
        let far_offset = ((far_point.1 as i32 - menu.bounds.y) as usize * CONTEXT_MENU_SIZE.width as usize
            + (far_point.0 as i32 - menu.bounds.x) as usize)
            * 4;
        let far_pixel = [pixels[far_offset + 2], pixels[far_offset + 1], pixels[far_offset], pixels[far_offset + 3]];
        for channel in 0..3 {
            assert!((i16::from(far_pixel[channel]) - i16::from(expected[channel])).abs() <= 1);
        }
        assert_eq!(far_pixel[3], 255);
    }

    #[test]
    fn wide_draft_strokes_extend_damage_bounds() {
        let selection = RectI { x: 0, y: 0, width: 500, height: 500 };
        let draft =
            DraftAnnotation::Rectangle { start: (50.0, 60.0), end: (200.0, 220.0), width: 12, color: DEFAULT_COLOR };
        let regions = draft_damage_regions(Some(&draft), None, selection);
        assert!(regions[0].y <= 60 - 6);
        assert!(regions[2].x <= 50 - 6);
        let pixels = render_draft_patch(regions[0], Some(&draft), None, false);
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn different_stroke_widths_render_differently() {
        let region = RectI { x: 5, y: 5, width: 70, height: 70 };
        let thin = [Annotation::Rectangle { start: (20.0, 20.0), end: (50.0, 50.0), width: 1, color: DEFAULT_COLOR }];
        let thick = [Annotation::Rectangle { start: (20.0, 20.0), end: (50.0, 50.0), width: 12, color: DEFAULT_COLOR }];
        let a = render_annotations_patch(region, &thin);
        let b = render_annotations_patch(region, &thick);
        assert_ne!(a, b);
        let visible = |pixels: &[u8]| pixels.chunks_exact(4).filter(|p| p[3] != 0).count();
        assert!(visible(&b) > visible(&a));
    }

    #[test]
    fn font_size_changes_text_damage_bounds() {
        let mut small = TextDraft::with_font_size((20.0, 20.0), 16);
        small.text = "中文ABC".into();
        let mut large = small.clone();
        large.font_size = 48;
        assert!(text_damage_rect(&large).width > text_damage_rect(&small).width);
        assert!(text_damage_rect(&large).height > text_damage_rect(&small).height);
    }

    #[test]
    fn hit_testing_finds_existing_pen_arrow_rectangle_text_and_mosaic() {
        let color = PALETTE[6].1;
        let ann = [
            Annotation::Pen { points: vec![(10.0, 10.0), (90.0, 10.0)], width: 3, color },
            Annotation::Arrow { start: (10.0, 20.0), end: (90.0, 20.0), width: 3, color },
            Annotation::Rectangle { start: (10.0, 35.0), end: (90.0, 95.0), width: 3, color },
            Annotation::Text { position: (110.0, 10.0), text: "中文Test".into(), font_size: 24, color },
            Annotation::Mosaic { start: (110.0, 40.0), end: (160.0, 100.0), block_size: 18 },
        ];
        assert!(annotation_hit_test(&ann[0], (30.0, 10.0)));
        assert!(annotation_hit_test(&ann[1], (55.0, 20.0)));
        assert!(annotation_hit_test(&ann[2], (50.0, 65.0)));
        assert!(annotation_hit_test(&ann[3], (120.0, 15.0)));
        assert!(annotation_hit_test(&ann[4], (135.0, 65.0)));
        assert!(!annotation_hit_test(&ann[0], (30.0, 120.0)));
        assert!(!annotation_hit_test(&ann[1], (55.0, 120.0)));
    }

    #[test]
    fn translating_annotations_preserves_styles() {
        let color = PALETTE[7].1;
        let variants = [
            Annotation::Pen { points: vec![(10.0, 20.0), (50.0, 60.0)], width: 8, color },
            Annotation::Arrow { start: (15.0, 10.0), end: (55.0, 35.0), width: 5, color },
            Annotation::Rectangle { start: (15.0, 25.0), end: (55.0, 85.0), width: 12, color },
            Annotation::Text { position: (30.0, 50.0), text: "中文Test".into(), font_size: 40, color },
            Annotation::Mosaic { start: (100.0, 100.0), end: (150.0, 150.0), block_size: 24 },
        ];
        for initial in &variants {
            let moved = translate_annotation(initial, 12.0, 7.0);
            assert_ne!(&moved, initial);
            assert_eq!(translate_annotation(&moved, -12.0, -7.0), *initial);
        }
    }

    #[test]
    fn palette_color_picker_is_leftmost_and_renders_new_choice() {
        let selection = RectI { x: 100, y: 100, width: 350, height: 200 };
        let size = PixelSize { width: 1024, height: 768 };
        let toolbar = toolbar_layout(selection, size);
        assert_eq!(toolbar.buttons[0].0, ToolbarAction::Color);
        assert!(toolbar.buttons.iter().any(|(a, _)| *a == ToolbarAction::Tool(EditorTool::Mosaic)));
        let a = render_toolbar_patch_with_color(&toolbar, EditorTool::Pen, PALETTE[0].1);
        let b = render_toolbar_patch_with_color(&toolbar, EditorTool::Pen, PALETTE[6].1);
        assert_ne!(a, b);
        let menu = color_menu_layout((300.0, 250.0), size);
        assert_eq!(menu.kind, ContextMenuKind::Color);
        assert_eq!(menu.options.len(), PALETTE.len() + 1);
        assert_eq!(menu.options.last().unwrap().0, PALETTE.len() as i32);
        assert_ne!(
            render_context_menu_patch(&menu, 0, DEFAULT_COLOR, None),
            render_context_menu_patch(&menu, 6, DEFAULT_COLOR, None)
        );
        assert_ne!(
            render_context_menu_patch(&menu, PALETTE.len() as i32, PALETTE[4].1, None),
            render_context_menu_patch(&menu, PALETTE.len() as i32, PALETTE[8].1, None)
        );
    }

    #[test]
    fn hex_custom_color_validates_six_digits_and_rgb_preview() {
        assert_eq!(parse_hex_color("#12A0fF"), Some([0x12, 0xA0, 0xFF, 255]));
        assert_eq!(parse_hex_color("abcdef"), Some([0xAB, 0xCD, 0xEF, 255]));
        assert_eq!(hex_color([0x0A, 0xB0, 0xFF, 255]), "0AB0FF");
        assert_eq!(parse_hex_color("#abc"), None);
        assert_eq!(parse_hex_color("1234567"), None);
        assert_eq!(parse_hex_color("GG1234"), None);
        let mut editor = CustomColorEditor::new(PALETTE[0].1);
        editor.start_hex_input();
        assert!(editor.chosen_color().is_none());
        assert!(!editor.push_hex('z'));
        for ch in "12a0fF".chars() {
            assert!(editor.push_hex(ch));
        }
        assert_eq!(editor.hex_input, "12A0FF");
        assert_eq!(editor.chosen_color(), Some([0x12, 0xA0, 0xFF, 255]));
        assert!(!editor.push_hex('0'));
        assert!(editor.backspace_hex());
        assert!(editor.chosen_color().is_none());
        assert!(editor.push_hex('f'));
        assert_eq!(editor.chosen_color(), Some([0x12, 0xA0, 0xFF, 255]));
        assert!(editor.set_channel(0, 255));
        assert_eq!(editor.hex_input, "FFA0FF");
        assert!(!editor.editing_hex);
        assert_eq!(editor.chosen_color(), Some([255, 160, 255, 255]));
    }

    #[test]
    fn custom_color_picker_hit_testing_and_rgb_sliders() {
        let screen = PixelSize { width: 1024, height: 768 };
        let menu = custom_color_menu_layout((1000.0, 740.0), screen);
        assert_eq!(menu.kind, ContextMenuKind::CustomColor);
        assert!(menu.bounds.x + menu.bounds.width <= 1024);
        assert!(menu.bounds.y + menu.bounds.height <= 768);
        for channel in 0..3 {
            let rect = menu.options[channel].1;
            assert_eq!(menu.hit_test((f64::from(rect.x + 68), f64::from(rect.y + 12))), Some(channel as i32));
            assert_eq!(custom_slider_value(&menu, channel, f64::from(rect.x + 68)), Some(0));
            assert_eq!(custom_slider_value(&menu, channel, f64::from(rect.x + rect.width - 10)), Some(255));
            assert_eq!(custom_slider_value(&menu, channel, f64::from(rect.x - 100)), Some(0));
            assert_eq!(custom_slider_value(&menu, channel, f64::from(rect.x + 500)), Some(255));
        }
        let editor = CustomColorEditor::new([19, 134, 221, 255]);
        let a = render_context_menu_patch(&menu, PALETTE.len() as i32, editor.color, Some(&editor));
        let mut changed = editor.clone();
        changed.set_channel(1, 255);
        let b = render_context_menu_patch(&menu, PALETTE.len() as i32, changed.color, Some(&changed));
        assert_ne!(a, b);
        assert_eq!(a.len(), CONTEXT_MENU_SIZE.width as usize * CONTEXT_MENU_SIZE.height as usize * 4);
    }

    #[test]
    fn hsv_conversion_and_custom_color_controls_stay_synchronized() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), [255, 0, 0, 255]);
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), [0, 255, 0, 255]);
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), [0, 0, 255, 255]);
        assert_eq!(hsv_to_rgb(300.0, 1.0, 1.0), [255, 0, 255, 255]);
        assert_eq!(hsv_to_rgb(60.0, 0.0, 1.0), [255, 255, 255, 255]);
        assert_eq!(hsv_to_rgb(60.0, 1.0, 0.0), [0, 0, 0, 255]);
        assert_eq!(rgb_to_hsv([0, 255, 0, 255]), (120.0, 1.0, 1.0));
        assert_eq!(rgb_to_hsv([128, 128, 128, 255]).1, 0.0);

        let mut editor = CustomColorEditor::new([255, 0, 0, 255]);
        assert!(editor.set_hue(120.0));
        assert_eq!(editor.color, [0, 255, 0, 255]);
        assert_eq!(editor.hex_input, "00FF00");
        assert!(editor.set_saturation_value(0.0, 1.0));
        assert_eq!(editor.color, [255, 255, 255, 255]);
        assert!(editor.set_hue(240.0));
        assert_eq!(editor.color, [255, 255, 255, 255]);
        assert!(editor.set_saturation_value(1.0, 1.0));
        assert_eq!(editor.color, [0, 0, 255, 255]);
        assert!(editor.set_channel(0, 255));
        assert_eq!(editor.hex_input, "FF00FF");
        assert!((editor.hue - 300.0).abs() < 0.01);
        editor.start_hex_input();
        for ch in "00FF00".chars() {
            assert!(editor.push_hex(ch));
        }
        assert_eq!(editor.color, [0, 255, 0, 255]);
        assert_eq!(editor.hue, 120.0);
        assert!(!editor.set_saturation_value(1.0, 1.0) || !editor.editing_hex);
        assert!(!editor.editing_hex);
    }

    #[test]
    fn saturation_value_square_and_hue_strip_render_real_gradients() {
        let screen = PixelSize { width: 1024, height: 768 };
        let menu = custom_color_menu_layout((130.0, 90.0), screen);
        let square = menu.options.iter().find(|(id, _)| *id == CUSTOM_SV_ID).unwrap().1;
        let hue = menu.options.iter().find(|(id, _)| *id == CUSTOM_HUE_ID).unwrap().1;
        assert_eq!(menu.hit_test((f64::from(square.x + 50), f64::from(square.y + 50))), Some(CUSTOM_SV_ID));
        assert_eq!(menu.hit_test((f64::from(hue.x + 50), f64::from(hue.y + 5))), Some(CUSTOM_HUE_ID));
        assert_eq!(custom_sat_value_at(&menu, (f64::from(square.x), f64::from(square.y))), Some((0.0, 1.0)));
        assert_eq!(
            custom_sat_value_at(
                &menu,
                (f64::from(square.x + square.width - 1), f64::from(square.y + square.height - 1))
            ),
            Some((1.0, 0.0))
        );
        assert_eq!(custom_hue_at(&menu, f64::from(hue.x)), Some(0.0));
        assert_eq!(custom_hue_at(&menu, f64::from(hue.x + hue.width - 1)), Some(360.0));
        assert_eq!(custom_hue_at(&menu, f64::from(hue.x - 300)), Some(0.0));
        assert_eq!(custom_hue_at(&menu, f64::from(hue.x + hue.width + 300)), Some(360.0));

        let editor = CustomColorEditor::new([255, 0, 0, 255]);
        let pixels = render_context_menu_patch(&menu, PALETTE.len() as i32, editor.color, Some(&editor));
        let width = CONTEXT_MENU_SIZE.width as usize;
        let sample = |x: i32, y: i32| {
            let index = ((y - menu.bounds.y) as usize * width + (x - menu.bounds.x) as usize) * 4;
            [pixels[index + 2], pixels[index + 1], pixels[index], pixels[index + 3]]
        };
        assert_eq!(pixels.len(), width * CONTEXT_MENU_SIZE.height as usize * 4);
        assert_eq!(sample(square.x, square.y), [255, 255, 255, 255]);
        let red = sample(square.x + square.width - 11, square.y + 3);
        assert!(red[0] > 235 && red[1] < 40 && red[2] < 40);
        assert_eq!(sample(square.x + 70, square.y + square.height - 1), [0, 0, 0, 255]);
        let green = sample(hue.x + (hue.width - 1) / 3, hue.y + 4);
        let blue = sample(hue.x + (hue.width - 1) * 2 / 3, hue.y + 4);
        assert!(green[1] > 245 && green[0] < 20 && green[2] < 20);
        assert!(blue[2] > 245 && blue[0] < 20 && blue[1] < 20);

        let normal_menu = color_menu_layout((100.0, 100.0), screen);
        assert_eq!(normal_menu.bounds.width, 190);
        assert_eq!(normal_menu.bounds.height, 296);
        let normal = render_context_menu_patch(&normal_menu, 0, DEFAULT_COLOR, None);
        let transparent_index = ((CONTEXT_MENU_SIZE.height as usize - 1) * width + width - 1) * 4;
        assert_eq!(normal.len(), pixels.len());
        assert_eq!(normal[transparent_index + 3], 0);
    }

    #[test]
    fn mosaic_export_replaces_original_pixels_and_keeps_outside_untouched() {
        let size = PixelSize { width: 48, height: 48 };
        let mut pixels = Vec::new();
        for y in 0..size.height {
            for x in 0..size.width {
                pixels.extend_from_slice(&[(x * 5 + y) as u8, (y * 4 + x) as u8, (x ^ y) as u8, 255]);
            }
        }
        let frame = CaptureFrame {
            screen: ScreenInfo {
                id: "DP-1".into(),
                name: "Display".into(),
                logical_geometry: None,
                pixel_size: size,
                scale_factor: Some(1.0),
            },
            image: RgbaImage::new(size, pixels).unwrap(),
            captured_at: SystemTime::now(),
        };
        let output = compose_annotated_crop(
            &frame,
            size,
            PixelRect { x: 0, y: 0, width: 48, height: 48 },
            &[Annotation::Mosaic { start: (8.0, 8.0), end: (40.0, 40.0), block_size: 16 }],
        )
        .unwrap();
        let at =
            |image: &RgbaImage, x: usize, y: usize| image.pixels()[(y * 48 + x) * 4..(y * 48 + x) * 4 + 4].to_vec();
        assert_eq!(at(&frame.image, 3, 3), at(&output, 3, 3));
        assert_ne!(at(&frame.image, 9, 9), at(&output, 9, 9));
        assert_eq!(at(&output, 9, 9), at(&output, 10, 10));
        assert_eq!(at(&output, 25, 25), at(&output, 26, 26));
    }

    #[test]
    fn keyboard_maps_basic_ascii() {
        assert_eq!(key_to_ascii(30, false), Some('a'));
        assert_eq!(key_to_ascii(30, true), Some('A'));
        assert_eq!(key_to_ascii(2, true), Some('!'));
        assert_eq!(key_to_ascii(57, false), Some(' '));
    }

    #[test]
    fn composing_annotations_changes_crop_pixels() {
        let frame = CaptureFrame {
            screen: ScreenInfo {
                id: "DP-1".into(),
                name: "Display".into(),
                logical_geometry: None,
                pixel_size: PixelSize { width: 20, height: 20 },
                scale_factor: Some(1.0),
            },
            image: RgbaImage::new(PixelSize { width: 20, height: 20 }, vec![0; 20 * 20 * 4]).unwrap(),
            captured_at: SystemTime::now(),
        };
        let annotations =
            vec![Annotation::Rectangle { start: (2.0, 2.0), end: (10.0, 10.0), width: 3, color: DEFAULT_COLOR }];
        let crop = compose_annotated_crop(
            &frame,
            PixelSize { width: 20, height: 20 },
            PixelRect { x: 0, y: 0, width: 20, height: 20 },
            &annotations,
        )
        .unwrap();
        assert!(crop.pixels().iter().any(|value| *value != 0));
    }
}
