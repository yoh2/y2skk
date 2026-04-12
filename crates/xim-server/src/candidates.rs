//! Candidate list window using X11 core protocol.
//!
//! Draws a floating vertical list of candidates in an override-redirect
//! window.  The focused candidate is highlighted with a blue background.
//! Positioned below the cursor (over-the-spot) when spot information is
//! available, falling back to the bottom-left of the screen.

use std::sync::Arc;

use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Char2b, ChangeGCAux, ConfigureWindowAux, ConnectionExt, CreateGCAux, CreateWindowAux,
    EventMask, Fontable, Rectangle, Screen, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

use crate::preedit::{SpotHint, FONT_CANDIDATES};

/// Normal row: light yellow background, black text.
const BG_COLOR: u32 = 0xFFFF_E0;
const FG_COLOR: u32 = 0x0000_00;
/// Focused row: blue background, white text.
const FOCUS_BG_COLOR: u32 = 0x4444_AA;
const FOCUS_FG_COLOR: u32 = 0xFFFF_FF;
const BORDER_COLOR: u32 = 0x6060_60;

const PADDING_X: i16 = 6;
const PADDING_Y: i16 = 3;
const BORDER_WIDTH: u16 = 1;

pub struct CandidateWindow {
    conn: Arc<RustConnection>,
    win: Window,
    gc: u32,
    screen_height: u16,
    screen_width: u16,
    font_ascent: i16,
    font_height: i16,
    mapped: bool,
}

impl CandidateWindow {
    pub fn new(conn: Arc<RustConnection>, screen: &Screen) -> Result<Self> {
        let font_id = conn.generate_id()?;
        let mut font_opened = false;
        for name in FONT_CANDIDATES {
            if conn.open_font(font_id, name.as_bytes()).is_ok() {
                conn.flush()?;
                match conn.query_font(font_id)?.reply() {
                    Ok(_) => {
                        tracing::info!("XIM candidates: opened font {name}");
                        font_opened = true;
                        break;
                    }
                    Err(_) => continue,
                }
            }
        }
        if !font_opened {
            anyhow::bail!("could not open any X11 font for candidate window");
        }

        let font_info = conn.query_font(font_id)?.reply()?;
        let font_ascent = font_info.font_ascent as i16;
        let font_descent = font_info.font_descent as i16;
        let font_height = font_ascent + font_descent;

        let win = conn.generate_id()?;
        let aux = CreateWindowAux::new()
            .override_redirect(1u32)
            .background_pixel(BG_COLOR)
            .border_pixel(BORDER_COLOR)
            .event_mask(EventMask::EXPOSURE)
            .save_under(1u32);
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            screen.root,
            0,
            0,
            1,
            1,
            BORDER_WIDTH,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &aux,
        )?;

        let gc = conn.generate_id()?;
        let gc_aux = CreateGCAux::new()
            .foreground(FG_COLOR)
            .background(BG_COLOR)
            .font(font_id);
        conn.create_gc(gc, win, &gc_aux)?;

        conn.close_font(font_id)?;
        conn.flush()?;

        Ok(Self {
            conn,
            win,
            gc,
            screen_height: screen.height_in_pixels,
            screen_width: screen.width_in_pixels,
            font_ascent,
            font_height,
            mapped: false,
        })
    }

    /// Shows the candidate list window.
    ///
    /// `candidates` — each entry is `"word"` or `"word;annotation"`.
    /// `sel_keys`   — one selection key character per candidate (e.g. `"asdfjkl;"`).
    /// `focused`    — index of the highlighted candidate.
    /// `spot`       — cursor position hint for positioning the window.
    pub fn show(
        &mut self,
        candidates: &[String],
        sel_keys: &str,
        focused: u32,
        spot: Option<&SpotHint>,
    ) -> Result<()> {
        if candidates.is_empty() {
            return self.hide();
        }

        let sel_chars: Vec<char> = sel_keys.chars().collect();
        let row_height = (self.font_height + PADDING_Y * 2) as u16;
        let win_height = row_height * candidates.len() as u16;

        // Build display strings and measure maximum width.
        let rows: Vec<Vec<Char2b>> = candidates
            .iter()
            .enumerate()
            .map(|(i, cand)| {
                let key = sel_chars.get(i).copied().unwrap_or(' ');
                // Split "word;annotation" into word and optional annotation.
                let (word, annotation) = match cand.split_once(';') {
                    Some((w, a)) => (w, Some(a)),
                    None => (cand.as_str(), None),
                };
                let label = match annotation {
                    Some(ann) => format!("{key}: {word}  ;{ann}"),
                    None => format!("{key}: {word}"),
                };
                str_to_char2b(&label)
            })
            .collect();

        // Find the widest row.
        let mut max_text_width: u16 = 0;
        for row in &rows {
            if row.is_empty() {
                continue;
            }
            if let Ok(reply) = self
                .conn
                .query_text_extents(Fontable::from(self.gc), row)?
                .reply()
            {
                let w = reply.overall_width.max(0) as u16;
                if w > max_text_width {
                    max_text_width = w;
                }
            }
        }
        let win_width = max_text_width + (PADDING_X as u16) * 2;

        // Compute position below the cursor.
        let (abs_x, abs_y) = self.compute_position(spot, win_width, win_height)?;

        // Clamp to screen bounds.
        let abs_x = abs_x.min(self.screen_width.saturating_sub(win_width + BORDER_WIDTH * 2) as i32).max(0);
        let abs_y = abs_y.min(self.screen_height.saturating_sub(win_height + BORDER_WIDTH * 2) as i32).max(0);

        let config = ConfigureWindowAux::new()
            .x(abs_x)
            .y(abs_y)
            .width(win_width as u32)
            .height(win_height as u32);
        self.conn.configure_window(self.win, &config)?;

        if !self.mapped {
            self.conn.map_window(self.win)?;
            self.mapped = true;
        }

        // Clear the window.
        self.conn.clear_area(true, self.win, 0, 0, win_width, win_height)?;

        // Draw each row.
        for (i, row) in rows.iter().enumerate() {
            let row_y = i as i16 * row_height as i16;
            let is_focused = i as u32 == focused;

            if is_focused {
                // Fill focused background.
                self.conn.change_gc(
                    self.gc,
                    &ChangeGCAux::new()
                        .foreground(FOCUS_BG_COLOR)
                        .background(FOCUS_BG_COLOR),
                )?;
                self.conn.poly_fill_rectangle(
                    self.win,
                    self.gc,
                    &[Rectangle {
                        x: 0,
                        y: row_y,
                        width: win_width,
                        height: row_height,
                    }],
                )?;
                // Draw focused text in white.
                self.conn.change_gc(
                    self.gc,
                    &ChangeGCAux::new()
                        .foreground(FOCUS_FG_COLOR)
                        .background(FOCUS_BG_COLOR),
                )?;
            } else {
                self.conn.change_gc(
                    self.gc,
                    &ChangeGCAux::new()
                        .foreground(FG_COLOR)
                        .background(BG_COLOR),
                )?;
            }

            if !row.is_empty() {
                self.conn.image_text16(
                    self.win,
                    self.gc,
                    PADDING_X,
                    row_y + PADDING_Y + self.font_ascent,
                    row,
                )?;
            }
        }

        // Raise to top.
        self.conn.configure_window(
            self.win,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        self.conn.flush()?;

        Ok(())
    }

    /// Hides the candidate window.
    pub fn hide(&mut self) -> Result<()> {
        if self.mapped {
            self.conn.unmap_window(self.win)?;
            self.conn.flush()?;
            self.mapped = false;
        }
        Ok(())
    }

    /// Computes the screen-absolute position of the candidate window.
    /// Places it below the cursor (spot.y + one font line), or falls back
    /// to the bottom-left of the screen.
    fn compute_position(
        &self,
        spot: Option<&SpotHint>,
        win_width: u16,
        win_height: u16,
    ) -> Result<(i32, i32)> {
        if let Some(hint) = spot {
            let root = self.root_window()?;
            if let Ok(r) = self
                .conn
                .translate_coordinates(hint.focus_win, root, hint.x, hint.y)?
                .reply()
            {
                // Place below the cursor line.
                let x = r.dst_x as i32;
                let y = r.dst_y as i32 + self.font_height as i32;
                return Ok((x, y));
            }
        }
        // Fallback: bottom-left, above the status bar area.
        let _ = (win_width, win_height);
        Ok((
            0,
            self.screen_height.saturating_sub(win_height + BORDER_WIDTH * 2) as i32,
        ))
    }

    fn root_window(&self) -> Result<Window> {
        let geom = self.conn.get_geometry(self.win)?.reply().context("get_geometry")?;
        Ok(geom.root)
    }
}

impl Drop for CandidateWindow {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.free_gc(self.gc);
    }
}

/// Converts a UTF-8 string to a slice of `Char2b` (big-endian UCS-2).
/// Characters outside the BMP are replaced with U+FFFD.
fn str_to_char2b(s: &str) -> Vec<Char2b> {
    s.chars()
        .map(|c| {
            let cp = if (c as u32) <= 0xFFFF { c as u32 } else { 0xFFFD };
            Char2b {
                byte1: ((cp >> 8) & 0xFF) as u8,
                byte2: (cp & 0xFF) as u8,
            }
        })
        .collect()
}
