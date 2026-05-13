use std::os::fd::{AsFd, OwnedFd};
use std::ptr;
use std::sync::Arc;

use fontdue::Font;
use wayland_client::{
    protocol::{wl_buffer, wl_compositor, wl_output, wl_shm, wl_shm_pool, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::wp::input_method::zv1::client::{
    zwp_input_panel_surface_v1::ZwpInputPanelSurfaceV1, zwp_input_panel_v1::ZwpInputPanelV1,
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

/// Candidate window surface dimensions.
const CAND_W: i32 = 380;
const CAND_H: i32 = LINE_H * MAX_ROWS as i32 + ROW_PAD * 2;

/// Status indicator surface dimensions used in overlay-panel mode (small and
/// square so the compositor can place it close to the cursor without
/// margin-shift artefacts).
const IND_H: i32 = LINE_H + ROW_PAD * 2;
const IND_W: i32 = IND_H;

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

// ── Canvas + SHM buffer ───────────────────────────────────────────────────────

/// A flat pixel buffer (ARGB8888, row stride = width * 4) plus its dimensions.
/// Drawing primitives operate on this so the same code can target surfaces of
/// different sizes.
struct Canvas<'a> {
    pixels: &'a mut [u8],
    width: i32,
    height: i32,
}

struct ShmBuf {
    fd: OwnedFd,
    #[allow(dead_code)] // pool must be kept alive for the buffer to remain valid
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    width: i32,
    height: i32,
}

impl ShmBuf {
    fn new(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<WaylandState>,
        width: i32,
        height: i32,
        tag: &str,
    ) -> anyhow::Result<Self> {
        let size = (width * height * 4) as usize;
        let fd = rustix::fs::memfd_create(tag, rustix::fs::MemfdFlags::empty())?;
        rustix::fs::ftruncate(&fd, size as u64)?;

        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );
        Ok(Self {
            fd,
            pool,
            buffer,
            width,
            height,
        })
    }
}

/// Map the memfd, build a Canvas over it, run `f`, then unmap. Used by all
/// rendering helpers below.
fn with_mapped_canvas<F: FnOnce(&mut Canvas<'_>)>(fd: &OwnedFd, width: i32, height: i32, f: F) {
    use rustix::mm::{mmap, munmap, MapFlags, ProtFlags};
    let size = (width * height * 4) as usize;
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
    let mut canvas = Canvas {
        pixels,
        width,
        height,
    };
    f(&mut canvas);
    unsafe { munmap(ptr as *mut _, size).expect("munmap") };
}

fn fill_transparent(canvas: &mut Canvas<'_>) {
    for chunk in canvas.pixels.chunks_exact_mut(4) {
        chunk.copy_from_slice(&TRANSPARENT);
    }
}

/// Overwrite the given memfd-backed buffer with fully-transparent pixels.
/// Used to "hide" a surface without releasing its proxy: we leave the
/// surface attached (so the compositor keeps managing it) but render
/// nothing visible into it.
fn render_transparent(fd: &OwnedFd, width: i32, height: i32) {
    with_mapped_canvas(fd, width, height, fill_transparent);
}

// ── Drawing primitives ────────────────────────────────────────────────────────

fn draw_text(
    canvas: &mut Canvas<'_>,
    font: &Font,
    text: &str,
    start_x: i32,
    baseline_y: i32,
    color: [u8; 4],
) {
    let mut pen_x = start_x as f32;
    for ch in text.chars() {
        if pen_x >= (canvas.width - H_MARGIN) as f32 {
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
                if !(0..canvas.width).contains(&px) || !(0..canvas.height).contains(&py) {
                    continue;
                }
                let off = (py * canvas.width * 4 + px * 4) as usize;
                blend_pixel(&mut canvas.pixels[off..off + 4], color, coverage);
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

fn draw_filled_rect(canvas: &mut Canvas<'_>, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    for row in y..(y + h) {
        if !(0..canvas.height).contains(&row) {
            continue;
        }
        for col in x..(x + w) {
            if !(0..canvas.width).contains(&col) {
                continue;
            }
            let off = (row * canvas.width * 4 + col * 4) as usize;
            canvas.pixels[off..off + 4].copy_from_slice(&color);
        }
    }
}

fn draw_rect(canvas: &mut Canvas<'_>, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    // Top and bottom edges.
    for col in x..(x + w) {
        if (0..canvas.width).contains(&col) {
            if (0..canvas.height).contains(&y) {
                let off = (y * canvas.width * 4 + col * 4) as usize;
                canvas.pixels[off..off + 4].copy_from_slice(&color);
            }
            let by = y + h - 1;
            if (0..canvas.height).contains(&by) {
                let off = (by * canvas.width * 4 + col * 4) as usize;
                canvas.pixels[off..off + 4].copy_from_slice(&color);
            }
        }
    }
    // Left and right edges.
    for row in y..(y + h) {
        if (0..canvas.height).contains(&row) {
            if (0..canvas.width).contains(&x) {
                let off = (row * canvas.width * 4 + x * 4) as usize;
                canvas.pixels[off..off + 4].copy_from_slice(&color);
            }
            let rx = x + w - 1;
            if (0..canvas.width).contains(&rx) {
                let off = (row * canvas.width * 4 + rx * 4) as usize;
                canvas.pixels[off..off + 4].copy_from_slice(&color);
            }
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render_candidates(
    fd: &OwnedFd,
    font: &Font,
    candidates: &[String],
    focused: u32,
    sel_keys: &str,
) {
    with_mapped_canvas(fd, CAND_W, CAND_H, |canvas| {
        fill_transparent(canvas);

        let key_chars: Vec<char> = sel_keys.chars().collect();
        let n = candidates.len().min(MAX_ROWS);
        let content_h = n as i32 * LINE_H + ROW_PAD * 2;

        draw_rect(canvas, 0, 0, CAND_W, content_h, BORDER_COLOR);
        draw_filled_rect(canvas, 1, 1, CAND_W - 2, content_h - 2, BG_COLOR);

        let baseline_offset = FONT_SIZE as i32 + ROW_PAD;
        for (i, cand) in candidates.iter().enumerate().take(n) {
            let row_y = ROW_PAD + i as i32 * LINE_H;
            if i == focused as usize {
                draw_filled_rect(canvas, 1, row_y, CAND_W - 2, LINE_H, FOCUS_BG);
            }
            let text = if i < key_chars.len() {
                format!("{}: {}", key_chars[i], cand)
            } else {
                cand.clone()
            };
            let baseline_y = row_y + baseline_offset;
            draw_text(canvas, font, &text, H_MARGIN, baseline_y, TEXT_COLOR);
        }
    });
}

/// Render the mode indicator into a small dedicated buffer (overlay mode).
/// The whole surface is the indicator box itself.
fn render_indicator_overlay(fd: &OwnedFd, font: &Font, indicator: &str) {
    with_mapped_canvas(fd, IND_W, IND_H, |canvas| {
        fill_transparent(canvas);
        draw_rect(canvas, 0, 0, IND_W, IND_H, BORDER_COLOR);
        draw_filled_rect(canvas, 1, 1, IND_W - 2, IND_H - 2, BG_COLOR);
        let baseline_y = ROW_PAD + FONT_SIZE as i32;
        draw_text(canvas, font, indicator, H_MARGIN, baseline_y, TEXT_COLOR);
    });
}

/// Render the mode indicator at the bottom-left of a candidate-sized buffer
/// (layer-shell fallback mode, where the surface itself is anchored to the
/// bottom-left of the screen).
fn render_indicator_legacy(fd: &OwnedFd, font: &Font, indicator: &str) {
    with_mapped_canvas(fd, CAND_W, CAND_H, |canvas| {
        fill_transparent(canvas);
        let box_w = IND_W;
        let box_h = IND_H;
        let box_x = 0;
        let box_y = CAND_H - box_h;
        draw_rect(canvas, box_x, box_y, box_w, box_h, BORDER_COLOR);
        draw_filled_rect(canvas, box_x + 1, box_y + 1, box_w - 2, box_h - 2, BG_COLOR);
        let baseline_y = box_y + ROW_PAD + FONT_SIZE as i32;
        draw_text(
            canvas,
            font,
            indicator,
            box_x + H_MARGIN,
            baseline_y,
            TEXT_COLOR,
        );
    });
}

// ── Surface backend ───────────────────────────────────────────────────────────

/// Backing surface management. `OverlayPanel` lets the compositor place the
/// surface near the application's text-input cursor; `LayerShell` anchors it
/// at the bottom-left of the screen as a fallback when `zwp_input_panel_v1`
/// is not advertised.
enum SurfaceBackend {
    LayerShell(ZwlrLayerSurfaceV1),
    // Held for the lifetime of the window — dropping the proxy destroys the
    // panel surface in the compositor.
    OverlayPanel(#[allow(dead_code)] ZwpInputPanelSurfaceV1),
}

impl SurfaceBackend {
    fn is_overlay(&self) -> bool {
        matches!(self, SurfaceBackend::OverlayPanel(_))
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
enum DisplayState {
    Empty,
    Candidates,
    Status,
}

pub struct CandidateWindow {
    surface: wl_surface::WlSurface,
    backend: SurfaceBackend,
    shm: wl_shm::WlShm,
    font: Arc<Font>,
    /// Buffer for the candidate list (always sized `CAND_W` × `CAND_H`).
    /// In layer-shell mode this buffer also hosts the indicator box at the
    /// bottom-left (legacy layout).
    cand_buf: Option<ShmBuf>,
    /// Small buffer for the mode indicator (only used in overlay-panel mode).
    ind_buf: Option<ShmBuf>,
    /// True when the surface is ready to receive content. For overlay panels
    /// this is set immediately (no configure handshake); for layer-shell it
    /// is set when the first Configure event arrives.
    configured: bool,
    pending: Option<PendingDraw>,
    state: DisplayState,
}

impl CandidateWindow {
    pub fn new(
        compositor: &wl_compositor::WlCompositor,
        layer_shell: Option<&ZwlrLayerShellV1>,
        input_panel: Option<&ZwpInputPanelV1>,
        shm: wl_shm::WlShm,
        font: Arc<Font>,
        qh: &QueueHandle<WaylandState>,
        output: Option<&wl_output::WlOutput>,
    ) -> Option<Self> {
        let surface = compositor.create_surface(qh, ());

        let (backend, configured) = if let Some(panel) = input_panel {
            tracing::info!("creating candidate window as input-panel overlay");
            let panel_surface = panel.get_input_panel_surface(&surface, qh, ());
            panel_surface.set_overlay_panel();
            // Overlay panel surfaces have no Configure handshake; we can draw
            // immediately after the initial commit below.
            (SurfaceBackend::OverlayPanel(panel_surface), true)
        } else if let Some(shell) = layer_shell {
            tracing::info!("creating candidate window via layer-shell (fallback)");
            let layer_surface = shell.get_layer_surface(
                &surface,
                output,
                zwlr_layer_shell_v1::Layer::Top,
                "y2skk-candidates".to_string(),
                qh,
                (),
            );
            layer_surface.set_anchor(Anchor::Bottom | Anchor::Left);
            // top, right, bottom, left — 40px from bottom, 20px from left.
            layer_surface.set_margin(0, 0, 40, 20);
            layer_surface.set_size(CAND_W as u32, CAND_H as u32);
            layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer_surface.set_exclusive_zone(0);
            (SurfaceBackend::LayerShell(layer_surface), false)
        } else {
            tracing::warn!("neither zwp_input_panel_v1 nor zwlr_layer_shell_v1 available");
            return None;
        };

        // Initial commit. For layer-shell this triggers the configure event;
        // for overlay panel it just registers the surface with the compositor.
        surface.commit();

        Some(Self {
            surface,
            backend,
            shm,
            font,
            cand_buf: None,
            ind_buf: None,
            configured,
            pending: None,
            state: DisplayState::Empty,
        })
    }

    /// Show candidates (takes priority over the status indicator).
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

    /// Hide candidates (or whatever else is currently on the surface).
    pub fn hide(&mut self) {
        if !self.configured {
            if matches!(self.pending, Some(PendingDraw::Candidates { .. })) {
                self.pending = None;
            }
            return;
        }
        if self.state == DisplayState::Candidates {
            self.fade_out_cand_buf();
            self.state = DisplayState::Empty;
        }
    }

    /// Show the mode indicator (no-op while candidates are visible).
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
            self.fade_out_ind_buf();
            self.state = DisplayState::Empty;
        }
    }

    /// Replace the currently-visible candidate buffer with a fully transparent
    /// payload and re-commit. `wl_surface::attach(None)` would be the
    /// canonical "no content" signal, but KWin keeps a stale input-panel
    /// overlay visible at its last (sometimes upward-shifted) position when
    /// it sees that and we don't get a real "hide" until the user moves
    /// focus, switches mode, or hovers the cursor over the leftover window.
    /// Attaching a transparent buffer of the same size and damaging the
    /// whole region forces KWin to render the surface again and the user
    /// sees it disappear immediately.
    fn fade_out_cand_buf(&mut self) {
        if let Some(buf) = self.cand_buf.as_ref() {
            render_transparent(&buf.fd, buf.width, buf.height);
            self.surface.attach(Some(&buf.buffer), 0, 0);
            self.surface.damage_buffer(0, 0, buf.width, buf.height);
            self.surface.commit();
        }
    }

    /// Same as `fade_out_cand_buf` but for the (smaller) indicator buffer
    /// in overlay-panel mode. In layer-shell fallback the indicator is
    /// drawn on the candidate-sized buffer, so this is a no-op then and
    /// `fade_out_cand_buf` is what runs from hide_status's caller chain
    /// — but here we keep the symmetry for the overlay-mode path.
    fn fade_out_ind_buf(&mut self) {
        if self.backend.is_overlay() {
            if let Some(buf) = self.ind_buf.as_ref() {
                render_transparent(&buf.fd, buf.width, buf.height);
                self.surface.attach(Some(&buf.buffer), 0, 0);
                self.surface.damage_buffer(0, 0, buf.width, buf.height);
                self.surface.commit();
            }
        } else if let Some(buf) = self.cand_buf.as_ref() {
            render_transparent(&buf.fd, buf.width, buf.height);
            self.surface.attach(Some(&buf.buffer), 0, 0);
            self.surface.damage_buffer(0, 0, buf.width, buf.height);
            self.surface.commit();
        }
    }

    fn do_draw_candidates(
        &mut self,
        candidates: &[String],
        focused: u32,
        sel_keys: &str,
        qh: &QueueHandle<WaylandState>,
    ) {
        if !self.ensure_cand_buf(qh) {
            return;
        }
        let buf = self.cand_buf.as_ref().unwrap();
        render_candidates(&buf.fd, &self.font, candidates, focused, sel_keys);
        self.attach_and_commit(buf.width, buf.height, true);
    }

    fn do_draw_status(&mut self, indicator: &str, qh: &QueueHandle<WaylandState>) {
        if self.backend.is_overlay() {
            // Overlay mode: dedicated small buffer so the surface size changes
            // to match the indicator, and the compositor places it tightly
            // near the cursor.
            if !self.ensure_ind_buf(qh) {
                return;
            }
            let buf = self.ind_buf.as_ref().unwrap();
            render_indicator_overlay(&buf.fd, &self.font, indicator);
            self.attach_and_commit(buf.width, buf.height, false);
        } else {
            // Layer-shell fallback: surface is screen-anchored at fixed size;
            // draw the indicator box at the bottom-left of the candidate-sized
            // buffer (legacy layout).
            if !self.ensure_cand_buf(qh) {
                return;
            }
            let buf = self.cand_buf.as_ref().unwrap();
            render_indicator_legacy(&buf.fd, &self.font, indicator);
            self.attach_and_commit(buf.width, buf.height, true);
        }
    }

    fn ensure_cand_buf(&mut self, qh: &QueueHandle<WaylandState>) -> bool {
        Self::ensure_buf(
            &self.shm,
            &mut self.cand_buf,
            qh,
            CAND_W,
            CAND_H,
            "y2skk-candidates",
        )
    }

    fn ensure_ind_buf(&mut self, qh: &QueueHandle<WaylandState>) -> bool {
        Self::ensure_buf(
            &self.shm,
            &mut self.ind_buf,
            qh,
            IND_W,
            IND_H,
            "y2skk-indicator",
        )
    }

    fn ensure_buf(
        shm: &wl_shm::WlShm,
        slot: &mut Option<ShmBuf>,
        qh: &QueueHandle<WaylandState>,
        width: i32,
        height: i32,
        tag: &str,
    ) -> bool {
        if slot.is_none() {
            match ShmBuf::new(shm, qh, width, height, tag) {
                Ok(b) => *slot = Some(b),
                Err(e) => {
                    tracing::error!("SHM buffer creation failed for {tag}: {e}");
                    return false;
                }
            }
        }
        true
    }

    /// Attach the buffer (caller already rendered into it) and commit.
    ///
    /// `is_cand_buf` chooses which buffer to attach without holding a mutable
    /// borrow of `self.surface` simultaneously with an immutable borrow of the
    /// buffer slot.
    fn attach_and_commit(&mut self, width: i32, height: i32, is_cand_buf: bool) {
        let buffer = if is_cand_buf {
            &self.cand_buf.as_ref().unwrap().buffer
        } else {
            &self.ind_buf.as_ref().unwrap().buffer
        };
        self.surface.attach(Some(buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
    }

    pub fn owns_layer_surface(&self, surface: &ZwlrLayerSurfaceV1) -> bool {
        match &self.backend {
            SurfaceBackend::LayerShell(ls) => ls.id() == surface.id(),
            SurfaceBackend::OverlayPanel(_) => false,
        }
    }

    pub fn handle_configure(&mut self, serial: u32, qh: &QueueHandle<WaylandState>) {
        if let SurfaceBackend::LayerShell(ls) = &self.backend {
            ls.ack_configure(serial);
        }
        let was_configured = self.configured;
        self.configured = true;
        if !was_configured {
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

// `zwp_input_panel_surface_v1` has no events, only requests.
impl Dispatch<ZwpInputPanelSurfaceV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpInputPanelSurfaceV1,
        _event: <ZwpInputPanelSurfaceV1 as Proxy>::Event,
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
