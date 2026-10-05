use lisnap_core::{CaptureError, CaptureFrame, PixelRect, PixelSize, RgbaImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EditorTool {
    #[default]
    Pen,
    Rectangle,
    Arrow,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolbarAction {
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
    Pen(Vec<(f64, f64)>),
    Rectangle { start: (f64, f64), end: (f64, f64) },
    Arrow { start: (f64, f64), end: (f64, f64) },
    Text { position: (f64, f64), text: String },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DraftAnnotation {
    Pen(Vec<(f64, f64)>),
    Rectangle { start: (f64, f64), end: (f64, f64) },
    Arrow { start: (f64, f64), end: (f64, f64) },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextDraft {
    pub(crate) position: (f64, f64),
    pub(crate) text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolbarLayout {
    pub(crate) bounds: RectI,
    pub(crate) buttons: Vec<(ToolbarAction, RectI)>,
}

const TOOLBAR_BUTTON: i32 = 40;
const TOOLBAR_GAP: i32 = 6;
const TOOLBAR_PAD: i32 = 8;
const TOOLBAR_HEIGHT: i32 = TOOLBAR_BUTTON + TOOLBAR_PAD * 2;
const TOOLBAR_COUNT: i32 = 8;

const ANNOTATION_COLOR: [u8; 4] = [255, 76, 76, 255];
const DRAFT_COLOR: [u8; 4] = [255, 124, 92, 220];
const TOOLBAR_BG: [u8; 4] = [24, 27, 32, 232];
const TOOLBAR_BUTTON_BG: [u8; 4] = [49, 54, 64, 236];
const TOOLBAR_ACTIVE_BG: [u8; 4] = [67, 92, 120, 244];
const TOOLBAR_ICON: [u8; 4] = [242, 245, 248, 255];
const TOOLBAR_CONFIRM: [u8; 4] = [93, 201, 128, 255];
const TOOLBAR_CANCEL: [u8; 4] = [242, 102, 102, 255];

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
        ToolbarAction::Tool(EditorTool::Pen),
        ToolbarAction::Tool(EditorTool::Rectangle),
        ToolbarAction::Tool(EditorTool::Arrow),
        ToolbarAction::Tool(EditorTool::Text),
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

pub(crate) fn render_editor_canvas(
    size: PixelSize,
    selection: RectI,
    tool: EditorTool,
    annotations: &[Annotation],
    draft: Option<&DraftAnnotation>,
    text_draft: Option<&TextDraft>,
) -> Vec<u8> {
    let mut raster = Raster::new(size);
    raster.clip = Some(selection);
    for annotation in annotations {
        draw_annotation(&mut raster, annotation, 1.0, 1.0, ANNOTATION_COLOR);
    }
    if let Some(draft) = draft {
        draw_draft(&mut raster, draft, 1.0, 1.0, DRAFT_COLOR);
    }
    if let Some(text) = text_draft {
        draw_text(&mut raster, text.position, &format!("{}_", text.text), 3, DRAFT_COLOR, 1.0, 1.0);
    }

    raster.clip = None;
    draw_toolbar(&mut raster, &toolbar_layout(selection, size), tool);
    rgba_to_wayland_argb(&raster.pixels)
}

pub(crate) fn compose_annotated_crop(
    frame: &CaptureFrame,
    logical_size: PixelSize,
    selection: PixelRect,
    annotations: &[Annotation],
) -> Result<RgbaImage, CaptureError> {
    let frame_size = frame.image.size();
    let mut raster = Raster { size: frame_size, pixels: frame.image.pixels().to_vec(), clip: None };
    let sx = f64::from(frame_size.width) / f64::from(logical_size.width);
    let sy = f64::from(frame_size.height) / f64::from(logical_size.height);
    raster.clip = Some(RectI {
        x: i32::try_from(selection.x).unwrap_or(i32::MAX),
        y: i32::try_from(selection.y).unwrap_or(i32::MAX),
        width: i32::try_from(selection.width).unwrap_or(i32::MAX),
        height: i32::try_from(selection.height).unwrap_or(i32::MAX),
    });
    for annotation in annotations {
        draw_annotation(&mut raster, annotation, sx, sy, ANNOTATION_COLOR);
    }
    RgbaImage::new(frame_size, raster.pixels)?.crop(selection)
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

fn draw_toolbar(raster: &mut Raster, toolbar: &ToolbarLayout, tool: EditorTool) {
    raster.fill_rect(toolbar.bounds, TOOLBAR_BG);
    for (action, rect) in &toolbar.buttons {
        let active = matches!(action, ToolbarAction::Tool(active_tool) if *active_tool == tool);
        raster.fill_rect(*rect, if active { TOOLBAR_ACTIVE_BG } else { TOOLBAR_BUTTON_BG });
        let cx = rect.x + rect.width / 2;
        let cy = rect.y + rect.height / 2;
        match action {
            ToolbarAction::Tool(EditorTool::Pen) => {
                raster.draw_line((cx - 11, cy + 9), (cx + 10, cy - 12), 3, TOOLBAR_ICON);
                raster.draw_disc(cx - 12, cy + 10, 3, TOOLBAR_ICON);
            }
            ToolbarAction::Tool(EditorTool::Rectangle) => {
                raster.draw_rect_outline(RectI { x: cx - 11, y: cy - 9, width: 22, height: 18 }, 2, TOOLBAR_ICON);
            }
            ToolbarAction::Tool(EditorTool::Arrow) => {
                raster.draw_line((cx - 12, cy + 8), (cx + 11, cy - 8), 2, TOOLBAR_ICON);
                raster.draw_line((cx + 11, cy - 8), (cx + 2, cy - 8), 2, TOOLBAR_ICON);
                raster.draw_line((cx + 11, cy - 8), (cx + 7, cy + 1), 2, TOOLBAR_ICON);
            }
            ToolbarAction::Tool(EditorTool::Text) => {
                raster.draw_text_bitmap((cx - 7, cy - 10), "T", 4, TOOLBAR_ICON);
            }
            ToolbarAction::Undo => {
                raster.draw_line((cx + 10, cy - 7), (cx - 7, cy - 7), 2, TOOLBAR_ICON);
                raster.draw_line((cx - 7, cy - 7), (cx - 13, cy), 2, TOOLBAR_ICON);
                raster.draw_line((cx - 13, cy), (cx - 7, cy + 7), 2, TOOLBAR_ICON);
            }
            ToolbarAction::Redo => {
                raster.draw_line((cx - 10, cy - 7), (cx + 7, cy - 7), 2, TOOLBAR_ICON);
                raster.draw_line((cx + 7, cy - 7), (cx + 13, cy), 2, TOOLBAR_ICON);
                raster.draw_line((cx + 13, cy), (cx + 7, cy + 7), 2, TOOLBAR_ICON);
            }
            ToolbarAction::Confirm => {
                raster.draw_line((cx - 11, cy), (cx - 3, cy + 8), 3, TOOLBAR_CONFIRM);
                raster.draw_line((cx - 3, cy + 8), (cx + 12, cy - 9), 3, TOOLBAR_CONFIRM);
            }
            ToolbarAction::Cancel => {
                raster.draw_line((cx - 9, cy - 9), (cx + 9, cy + 9), 3, TOOLBAR_CANCEL);
                raster.draw_line((cx + 9, cy - 9), (cx - 9, cy + 9), 3, TOOLBAR_CANCEL);
            }
        }
    }
}

fn draw_annotation(raster: &mut Raster, annotation: &Annotation, sx: f64, sy: f64, color: [u8; 4]) {
    match annotation {
        Annotation::Pen(points) => draw_pen(raster, points, sx, sy, color),
        Annotation::Rectangle { start, end } => {
            let start = transform_point(*start, sx, sy);
            let end = transform_point(*end, sx, sy);
            if let Some(rect) = rect_from_points(start, end) {
                raster.draw_rect_outline(rect, scaled_width(sx, sy, 3.0), color);
            }
        }
        Annotation::Arrow { start, end } => {
            draw_arrow(
                raster,
                transform_point(*start, sx, sy),
                transform_point(*end, sx, sy),
                scaled_width(sx, sy, 3.0),
                color,
            );
        }
        Annotation::Text { position, text } => {
            draw_text(raster, *position, text, 3, color, sx, sy);
        }
    }
}

fn draw_draft(raster: &mut Raster, draft: &DraftAnnotation, sx: f64, sy: f64, color: [u8; 4]) {
    match draft {
        DraftAnnotation::Pen(points) => draw_pen(raster, points, sx, sy, color),
        DraftAnnotation::Rectangle { start, end } => {
            let start = transform_point(*start, sx, sy);
            let end = transform_point(*end, sx, sy);
            if let Some(rect) = rect_from_points(start, end) {
                raster.draw_rect_outline(rect, scaled_width(sx, sy, 3.0), color);
            }
        }
        DraftAnnotation::Arrow { start, end } => {
            draw_arrow(
                raster,
                transform_point(*start, sx, sy),
                transform_point(*end, sx, sy),
                scaled_width(sx, sy, 3.0),
                color,
            );
        }
    }
}

fn draw_pen(raster: &mut Raster, points: &[(f64, f64)], sx: f64, sy: f64, color: [u8; 4]) {
    let width = scaled_width(sx, sy, 3.0);
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

fn draw_text(
    raster: &mut Raster,
    position: (f64, f64),
    text: &str,
    logical_scale: i32,
    color: [u8; 4],
    sx: f64,
    sy: f64,
) {
    let position = transform_point(position, sx, sy);
    let scale = ((f64::from(logical_scale) * (sx + sy) * 0.5).round() as i32).max(1);
    raster.draw_text_bitmap(position, text, scale, color);
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
    pixels: Vec<u8>,
    clip: Option<RectI>,
}

impl Raster {
    fn new(size: PixelSize) -> Self {
        let len = size.width as usize * size.height as usize * 4;
        Self { size, pixels: vec![0; len], clip: None }
    }

    fn blend_pixel(&mut self, x: i32, y: i32, color: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.size.width as i32 || y >= self.size.height as i32 {
            return;
        }
        if self.clip.is_some_and(|clip| !clip.contains((f64::from(x), f64::from(y)))) {
            return;
        }
        let index = (y as usize * self.size.width as usize + x as usize) * 4;
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
        for y in rect.y.max(0)..(rect.y + rect.height).min(self.size.height as i32) {
            for x in rect.x.max(0)..(rect.x + rect.width).min(self.size.width as i32) {
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

fn rgba_to_wayland_argb(rgba: &[u8]) -> Vec<u8> {
    let mut native = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        let a = u16::from(pixel[3]);
        let premul = |value: u8| ((u16::from(value) * a + 127) / 255) as u8;
        native.extend_from_slice(&[premul(pixel[2]), premul(pixel[1]), premul(pixel[0]), pixel[3]]);
    }
    native
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
        let (_, first) = layout.buttons[0];
        assert_eq!(
            layout.hit_test((f64::from(first.x + 2), f64::from(first.y + 2))),
            Some(ToolbarAction::Tool(EditorTool::Pen))
        );
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
        let annotations = vec![Annotation::Rectangle { start: (2.0, 2.0), end: (10.0, 10.0) }];
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
