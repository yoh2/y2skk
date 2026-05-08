use std::os::fd::{AsFd, OwnedFd};
use std::ptr;
use std::sync::Arc;

use fontdue::Font;
use wayland_client::{
    protocol::{wl_buffer, wl_compositor, wl_output, wl_shm, wl_shm_pool, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use crate::server::WaylandState;

// ── Layout constants ──────────────────────────────────────────────────────────

const FONT_SIZE: f32 = 16.0;
/// Vertical padding inside each candidate row (pixels above and below text).
const ROW_PAD: i32 = 4;
/// Horizontal margin inside the window.
const H_MARGIN: i32 = 8;
const LINE_H: i32 = FONT_SIZE as i32 + ROW_PAD * 2;
const MAX_ROWS: usize = 10;
/// Fixed canvas width in pixels.
const CANVAS_W: i32 = 380;
/// Maximum canvas height (enough for MAX_ROWS candidates).
const CANVAS_H: i32 = LINE_H * MAX_ROWS as i32 + ROW_PAD * 2;

// ARGB8888 little-endian: memory bytes are [B, G, R, A].
const BG_COLOR: [u8; 4] = [0xE0, 0xFF, 0xFF, 0xFF]; // #FFFFE0 — light yellow
const FOCUS_BG: [u8; 4] = [0xCC, 0xCC, 0xFF, 0xFF]; // #FFCCCC — light red/pink
const TEXT_COLOR: [u8; 4] = [0x00, 0x00, 0x00, 0xFF]; // black
const BORDER_COLOR: [u8; 4] = [0x80, 0x80, 0x80, 0xFF]; // grey border
const TRANSPARENT: [u8; 4] = [0x00, 0x00, 0x00, 0x00]; // fully transparent

// ── Font discovery ────────────────────────────────────────────────────────────

const FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/ipaex/ipaexg.ttf",
    "/usr/share/fonts/vlgothic/VL-Gothic-Regular.ttf",
    "/usr/share/fonts/ja-ipafonts/ipag.ttf",
    "/usr/share/fonts/ipamonafont/ipag-mona.ttf",
];

pub fn load_font() -> Option<Font> {
    for path in FONT_PATHS {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(font) = Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                tracing::info!("loaded font: {path}");
                return Some(font);
            }
        }
    }
    tracing::warn!("no CJK font found — candidate window disabled");
    None
}

// ── Pixel buffer helpers ──────────────────────────────────────────────────────

struct ShmBuf {
    fd: OwnedFd,
    #[allow(dead_code)] // pool must be kept alive for the buffer to remain valid
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
}

impl ShmBuf {
    fn new(shm: &wl_shm::WlShm, qh: &QueueHandle<WaylandState>) -> anyhow::Result<Self> {
        let size = (CANVAS_W * CANVAS_H * 4) as usize;
        let fd = rustix::fs::memfd_create("y2skk-cands", rustix::fs::MemfdFlags::empty())?;
        rustix::fs::ftruncate(&fd, size as u64)?;

        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            CANVAS_W,
            CANVAS_H,
            CANVAS_W * 4,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );
        Ok(Self { fd, pool, buffer })
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

/// Write ARGB pixel data to the memfd, then return.
fn render_candidates(
    fd: &OwnedFd,
    font: &Font,
    candidates: &[String],
    focused: u32,
    sel_keys: &str,
) {
    use rustix::mm::{mmap, munmap, MapFlags, ProtFlags};

    let size = (CANVAS_W * CANVAS_H * 4) as usize;
    let ptr = unsafe {
        mmap(
            ptr::null_mut(),
            size,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            fd,
            0,
        )
        .expect("mmap") as *mut u8
    };

    let pixels = unsafe { std::slice::from_raw_parts_mut(ptr, size) };

    // Fill entire canvas with transparent (hidden area stays invisible).
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.copy_from_slice(&TRANSPARENT);
    }

    let key_chars: Vec<char> = sel_keys.chars().collect();
    let n = candidates.len().min(MAX_ROWS);
    let content_h = n as i32 * LINE_H + ROW_PAD * 2;

    // Draw 1-pixel grey border.
    draw_rect(pixels, 0, 0, CANVAS_W, content_h, BORDER_COLOR, CANVAS_W);
    // Fill interior background.
    draw_filled_rect(
        pixels,
        1,
        1,
        CANVAS_W - 2,
        content_h - 2,
        BG_COLOR,
        CANVAS_W,
    );

    let baseline_offset = FONT_SIZE as i32 + ROW_PAD;

    for (i, cand) in candidates.iter().enumerate().take(n) {
        let row_y = ROW_PAD + i as i32 * LINE_H;

        // Focused row highlight.
        if i == focused as usize {
            draw_filled_rect(pixels, 1, row_y, CANVAS_W - 2, LINE_H, FOCUS_BG, CANVAS_W);
        }

        // Build "a: 候補" label.
        let text = if i < key_chars.len() {
            format!("{}: {}", key_chars[i], cand)
        } else {
            cand.clone()
        };

        let baseline_y = row_y + baseline_offset;
        draw_text(
            pixels, font, &text, H_MARGIN, baseline_y, TEXT_COLOR, CANVAS_W, CANVAS_H,
        );
    }

    unsafe { munmap(ptr as *mut _, size).expect("munmap") };
}

/// Render the mode indicator as a small box at the bottom-left of the canvas.
fn render_status(fd: &OwnedFd, font: &Font, indicator: &str) {
    use rustix::mm::{mmap, munmap, MapFlags, ProtFlags};

    let size = (CANVAS_W * CANVAS_H * 4) as usize;
    let ptr = unsafe {
        mmap(
            ptr::null_mut(),
            size,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            fd,
            0,
        )
        .expect("mmap") as *mut u8
    };
    let pixels = unsafe { std::slice::from_raw_parts_mut(ptr, size) };

    // Clear the full canvas first.
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.copy_from_slice(&TRANSPARENT);
    }

    // Small indicator box (~50x30) anchored to bottom-left of the canvas
    // (which itself is anchored to bottom-left of the screen).
    let box_w: i32 = 60;
    let box_h: i32 = LINE_H + ROW_PAD * 2;
    let box_x: i32 = 0;
    let box_y: i32 = CANVAS_H - box_h;

    draw_rect(pixels, box_x, box_y, box_w, box_h, BORDER_COLOR, CANVAS_W);
    draw_filled_rect(
        pixels,
        box_x + 1,
        box_y + 1,
        box_w - 2,
        box_h - 2,
        BG_COLOR,
        CANVAS_W,
    );

    let baseline_y = box_y + ROW_PAD + FONT_SIZE as i32;
    draw_text(
        pixels,
        font,
        indicator,
        box_x + H_MARGIN,
        baseline_y,
        TEXT_COLOR,
        CANVAS_W,
        CANVAS_H,
    );

    unsafe { munmap(ptr as *mut _, size).expect("munmap") };
}

// TODO: reduce the number of function arguments
fn draw_text(
    pixels: &mut [u8],
    font: &Font,
    text: &str,
    start_x: i32,
    baseline_y: i32,
    color: [u8; 4],
    stride_px: i32,
    canvas_h: i32,
) {
    let mut pen_x = start_x as f32;
    for ch in text.chars() {
        if pen_x >= (CANVAS_W - H_MARGIN) as f32 {
            break;
        }
        let (metrics, bitmap) = font.rasterize(ch, FONT_SIZE);
        // Bitmap top-left in screen coords: x = pen + xmin, y = baseline - ymin - height
        let glyph_x = pen_x as i32 + metrics.xmin;
        let glyph_y = baseline_y - metrics.ymin - metrics.height as i32;

        for by in 0..metrics.height {
            for bx in 0..metrics.width {
                let coverage = bitmap[by * metrics.width + bx];
                if coverage == 0 {
                    continue;
                }
                let px = glyph_x + bx as i32;
                let py = glyph_y + by as i32;
                if !(0..CANVAS_W).contains(&px) || !(0..canvas_h).contains(&py) {
                    continue;
                }
                let off = (py * stride_px * 4 + px * 4) as usize;
                blend_pixel(&mut pixels[off..off + 4], color, coverage);
            }
        }
        pen_x += metrics.advance_width;
    }
}

/// Alpha-blend a foreground pixel (with given coverage) over the existing pixel.
#[inline]
fn blend_pixel(dst: &mut [u8], fg: [u8; 4], coverage: u8) {
    let a = coverage as u32;
    let ia = 255 - a;
    dst[0] = ((fg[0] as u32 * a + dst[0] as u32 * ia) / 255) as u8; // B
    dst[1] = ((fg[1] as u32 * a + dst[1] as u32 * ia) / 255) as u8; // G
    dst[2] = ((fg[2] as u32 * a + dst[2] as u32 * ia) / 255) as u8; // R
    dst[3] = 0xFF;
}

fn draw_filled_rect(
    pixels: &mut [u8],
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: [u8; 4],
    stride_px: i32,
) {
    for row in y..(y + h) {
        if !(0..CANVAS_H).contains(&row) {
            continue;
        }
        for col in x..(x + w) {
            if !(0..CANVAS_W).contains(&col) {
                continue;
            }
            let off = (row * stride_px * 4 + col * 4) as usize;
            pixels[off..off + 4].copy_from_slice(&color);
        }
    }
}

fn draw_rect(pixels: &mut [u8], x: i32, y: i32, w: i32, h: i32, color: [u8; 4], stride_px: i32) {
    // Top and bottom edges.
    for col in x..(x + w) {
        if (0..CANVAS_W).contains(&col) {
            if (0..CANVAS_H).contains(&y) {
                let off = (y * stride_px * 4 + col * 4) as usize;
                pixels[off..off + 4].copy_from_slice(&color);
            }
            let by = y + h - 1;
            if (0..CANVAS_H).contains(&by) {
                let off = (by * stride_px * 4 + col * 4) as usize;
                pixels[off..off + 4].copy_from_slice(&color);
            }
        }
    }
    // Left and right edges.
    for row in y..(y + h) {
        if (0..CANVAS_H).contains(&row) {
            if (0..CANVAS_W).contains(&x) {
                let off = (row * stride_px * 4 + x * 4) as usize;
                pixels[off..off + 4].copy_from_slice(&color);
            }
            let rx = x + w - 1;
            if (0..CANVAS_W).contains(&rx) {
                let off = (row * stride_px * 4 + rx * 4) as usize;
                pixels[off..off + 4].copy_from_slice(&color);
            }
        }
    }
}

// ── CandidateWindow ───────────────────────────────────────────────────────────

/// Pending draw queued while the layer surface is not yet configured.
enum PendingDraw {
    Candidates {
        candidates: Vec<String>,
        focused: u32,
        sel_keys: String,
    },
    Status {
        indicator: String,
    },
}

/// What's currently on the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayState {
    Empty,
    Candidates,
    Status,
}

pub struct CandidateWindow {
    surface: wl_surface::WlSurface,
    layer_surface: ZwlrLayerSurfaceV1,
    shm: wl_shm::WlShm,
    font: Arc<Font>,
    buf: Option<ShmBuf>,
    configured: bool,
    pending: Option<PendingDraw>,
    state: DisplayState,
}

impl CandidateWindow {
    pub fn new(
        compositor: &wl_compositor::WlCompositor,
        layer_shell: &ZwlrLayerShellV1,
        shm: wl_shm::WlShm,
        font: Arc<Font>,
        qh: &QueueHandle<WaylandState>,
        output: Option<&wl_output::WlOutput>,
    ) -> Self {
        let surface = compositor.create_surface(qh, ());

        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            output,
            zwlr_layer_shell_v1::Layer::Top,
            "y2skk-candidates".to_string(),
            qh,
            (),
        );

        layer_surface.set_anchor(Anchor::Bottom | Anchor::Left);
        layer_surface.set_margin(0, 0, 40, 20); // bottom, right, top, left → 40px from bottom, 20px from left
        layer_surface.set_size(CANVAS_W as u32, CANVAS_H as u32);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_exclusive_zone(0);

        // Initial commit triggers the configure event.
        surface.commit();

        Self {
            surface,
            layer_surface,
            shm,
            font,
            buf: None,
            configured: false,
            pending: None,
            state: DisplayState::Empty,
        }
    }

    /// Show candidates (takes priority over status indicator).
    pub fn show(
        &mut self,
        candidates: &[String],
        focused: u32,
        sel_keys: &str,
        qh: &QueueHandle<WaylandState>,
    ) {
        if !self.configured {
            self.pending = Some(PendingDraw::Candidates {
                candidates: candidates.to_vec(),
                focused,
                sel_keys: sel_keys.to_string(),
            });
            return;
        }
        self.do_draw_candidates(candidates, focused, sel_keys, qh);
        self.state = DisplayState::Candidates;
    }

    /// Hide candidates (clears the surface).
    pub fn hide(&mut self) {
        if !self.configured {
            if matches!(self.pending, Some(PendingDraw::Candidates { .. })) {
                self.pending = None;
            }
            return;
        }
        if self.state == DisplayState::Candidates {
            self.clear_surface();
            self.state = DisplayState::Empty;
        }
    }

    /// Show the mode indicator ("あ", "A", ...) — only when candidates are not visible.
    pub fn show_status(&mut self, indicator: &str, qh: &QueueHandle<WaylandState>) {
        if self.state == DisplayState::Candidates {
            return;
        }
        if !self.configured {
            self.pending = Some(PendingDraw::Status {
                indicator: indicator.to_string(),
            });
            return;
        }
        self.do_draw_status(indicator, qh);
        self.state = DisplayState::Status;
    }

    /// Hide the mode indicator (called when the auto-hide timer fires).
    pub fn hide_status(&mut self) {
        if !self.configured {
            if matches!(self.pending, Some(PendingDraw::Status { .. })) {
                self.pending = None;
            }
            return;
        }
        if self.state == DisplayState::Status {
            self.clear_surface();
            self.state = DisplayState::Empty;
        }
    }

    fn clear_surface(&mut self) {
        if let Some(buf) = self.buf.as_ref() {
            let _ = self.make_transparent();
            self.surface.attach(Some(&buf.buffer), 0, 0);
            self.surface.damage_buffer(0, 0, CANVAS_W, CANVAS_H);
            self.surface.commit();
        }
    }

    fn make_transparent(&self) -> anyhow::Result<()> {
        use rustix::mm::{mmap, munmap, MapFlags, ProtFlags};
        let buf = self.buf.as_ref().unwrap();
        let size = (CANVAS_W * CANVAS_H * 4) as usize;
        let ptr = unsafe {
            mmap(
                ptr::null_mut(),
                size,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &buf.fd,
                0,
            )? as *mut u8
        };
        let pixels = unsafe { std::slice::from_raw_parts_mut(ptr, size) };
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&TRANSPARENT);
        }
        unsafe { munmap(ptr as *mut _, size)? };
        Ok(())
    }

    fn do_draw_candidates(
        &mut self,
        candidates: &[String],
        focused: u32,
        sel_keys: &str,
        qh: &QueueHandle<WaylandState>,
    ) {
        if !self.ensure_buffer(qh) {
            return;
        }
        render_candidates(
            &self.buf.as_ref().unwrap().fd,
            &self.font,
            candidates,
            focused,
            sel_keys,
        );
        self.commit_buffer();
    }

    fn do_draw_status(&mut self, indicator: &str, qh: &QueueHandle<WaylandState>) {
        if !self.ensure_buffer(qh) {
            return;
        }
        render_status(&self.buf.as_ref().unwrap().fd, &self.font, indicator);
        self.commit_buffer();
    }

    fn ensure_buffer(&mut self, qh: &QueueHandle<WaylandState>) -> bool {
        if self.buf.is_none() {
            match ShmBuf::new(&self.shm, qh) {
                Ok(b) => self.buf = Some(b),
                Err(e) => {
                    tracing::error!("SHM buffer creation failed: {e}");
                    return false;
                }
            }
        }
        true
    }

    fn commit_buffer(&mut self) {
        let buf = self.buf.as_ref().unwrap();
        self.surface.attach(Some(&buf.buffer), 0, 0);
        self.surface.damage_buffer(0, 0, CANVAS_W, CANVAS_H);
        self.surface.commit();
    }

    /// Returns true if this window owns the given layer surface proxy.
    pub fn owns_layer_surface(&self, surface: &ZwlrLayerSurfaceV1) -> bool {
        self.layer_surface.id() == surface.id()
    }

    /// Called by the event dispatcher when a configure event is received.
    pub fn handle_configure(&mut self, serial: u32, qh: &QueueHandle<WaylandState>) {
        self.layer_surface.ack_configure(serial);
        self.configured = true;

        match self.pending.take() {
            Some(PendingDraw::Candidates {
                candidates,
                focused,
                sel_keys,
            }) => {
                self.do_draw_candidates(&candidates, focused, &sel_keys, qh);
                self.state = DisplayState::Candidates;
            }
            Some(PendingDraw::Status { indicator }) => {
                self.do_draw_status(&indicator, qh);
                self.state = DisplayState::Status;
            }
            None => {}
        }
    }
}

// ── Dispatch implementations ──────────────────────────────────────────────────

impl Dispatch<ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width: _,
                height: _,
            } => {
                for cw in &mut state.candidate_windows {
                    if cw.owns_layer_surface(proxy) {
                        cw.handle_configure(serial, qh);
                        break;
                    }
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                tracing::warn!("layer surface closed — removing window for that output");
                state
                    .candidate_windows
                    .retain(|cw| !cw.owns_layer_surface(proxy));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrLayerShellV1,
        _event: zwlr_layer_shell_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm_pool::WlShmPool,
        _event: wl_shm_pool::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_buffer::WlBuffer,
        _event: wl_buffer::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}
