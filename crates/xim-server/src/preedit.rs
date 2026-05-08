//! Preedit window using X11 core protocol.
//!
//! Draws preedit text in an override-redirect window.  When the client
//! provides a spot location and focus window (over-the-spot), the window
//! is positioned at the cursor.  Otherwise it falls back to the bottom-left
//! of the screen (root-window style).

use std::sync::Arc;

use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Char2b, ConfigureWindowAux, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Fontable,
    Screen, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

/// Font candidates in preference order.  The first one that opens
/// successfully wins.
pub const FONT_CANDIDATES: &[&str] = &[
    "-misc-fixed-medium-r-normal-ja-18-120-100-100-c-180-iso10646-1",
    "-misc-fixed-medium-r-normal-ja-13-120-75-75-c-120-iso10646-1",
    "-misc-fixed-medium-r-normal--14-130-75-75-c-70-iso10646-1",
    "fixed",
];

/// Background / foreground colours for the preedit window.
const BG_COLOR: u32 = 0xFF_FF_E0; // light yellow
const FG_COLOR: u32 = 0x00_00_00; // black
const GHOST_FG_COLOR: u32 = 0x80_80_80; // grey for completion ghost text
const BORDER_COLOR: u32 = 0x60_60_60; // dark grey

const PADDING: i16 = 4;
const BORDER_WIDTH: u16 = 1;

/// Hint from the IC about where to place the preedit window.
pub struct SpotHint {
    /// X coordinate of the cursor, relative to `focus_win`.
    pub x: i16,
    /// Y coordinate of the cursor, relative to `focus_win`.
    pub y: i16,
    /// The client's focus window (or client window as fallback).
    pub focus_win: Window,
}

pub struct PreeditWindow {
    conn: Arc<RustConnection>,
    win: Window,
    /// GC for user-typed text (black).
    gc: u32,
    /// GC for completion ghost text (grey).
    ghost_gc: u32,
    screen_height: u16,
    font_ascent: i16,
    font_descent: i16,
    mapped: bool,
}

impl PreeditWindow {
    pub fn new(conn: Arc<RustConnection>, screen: &Screen) -> Result<Self> {
        let font_id = conn.generate_id()?;
        let mut font_opened = false;
        for name in FONT_CANDIDATES {
            if conn.open_font(font_id, name.as_bytes()).is_ok() {
                conn.flush()?;
                // Verify the font was actually opened by querying it.
                match conn.query_font(font_id)?.reply() {
                    Ok(_) => {
                        tracing::info!("XIM preedit: opened font {name}");
                        font_opened = true;
                        break;
                    }
                    Err(_) => continue,
                }
            }
        }
        if !font_opened {
            anyhow::bail!("could not open any X11 font for preedit window");
        }

        // Get font metrics.
        let font_info = conn.query_font(font_id)?.reply()?;
        let font_ascent = font_info.font_ascent;
        let font_descent = font_info.font_descent;

        // Create the preedit window (override-redirect, unmapped).
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

        // Create a GC for normal (user-typed) text.
        let gc = conn.generate_id()?;
        let gc_aux = CreateGCAux::new()
            .foreground(FG_COLOR)
            .background(BG_COLOR)
            .font(font_id);
        conn.create_gc(gc, win, &gc_aux)?;

        // Create a second GC for ghost (completion preview) text in grey.
        let ghost_gc = conn.generate_id()?;
        let ghost_gc_aux = CreateGCAux::new()
            .foreground(GHOST_FG_COLOR)
            .background(BG_COLOR)
            .font(font_id);
        conn.create_gc(ghost_gc, win, &ghost_gc_aux)?;

        // Close the font handle (GCs retain their own references).
        conn.close_font(font_id)?;
        conn.flush()?;

        Ok(Self {
            conn,
            win,
            gc,
            ghost_gc,
            screen_height: screen.height_in_pixels,
            font_ascent,
            font_descent,
            mapped: false,
        })
    }

    /// Shows the preedit window with the given text.
    ///
    /// `ghost_start` is the byte offset within `text` where the completion
    /// ghost preview begins.  Characters from that position onwards are drawn
    /// in grey; the rest are drawn in the normal foreground colour.
    ///
    /// If `spot` is provided, the window is placed at the cursor position
    /// (over-the-spot).  Otherwise it falls back to the bottom-left of the
    /// screen.
    pub fn update(
        &mut self,
        text: &str,
        ghost_start: Option<usize>,
        spot: Option<&SpotHint>,
    ) -> Result<()> {
        if text.is_empty() {
            return self.hide();
        }

        let all_chars: Vec<Char2b> = text
            .chars()
            .map(|c| {
                let cp = c as u32;
                Char2b {
                    byte1: ((cp >> 8) & 0xFF) as u8,
                    byte2: (cp & 0xFF) as u8,
                }
            })
            .collect();

        // Split at ghost boundary (convert byte offset to char count).
        let ghost_char_idx: Option<usize> = ghost_start.map(|byte_off| {
            let clamped = byte_off.min(text.len());
            text[..clamped].chars().count()
        });

        // Measure total text width.
        let extents = self
            .conn
            .query_text_extents(Fontable::from(self.gc), &all_chars)?
            .reply()
            .context("query_text_extents")?;
        let text_width = extents.overall_width as u16;
        let win_width = text_width + (PADDING as u16) * 2;
        let win_height = (self.font_ascent + self.font_descent) as u16 + (PADDING as u16) * 2;

        // Compute screen-absolute position.
        let (abs_x, abs_y) = match spot {
            Some(hint) => {
                // Translate the focus-window-relative spot to root coordinates.
                match self
                    .conn
                    .translate_coordinates(hint.focus_win, self.root_window()?, hint.x, hint.y)?
                    .reply()
                {
                    Ok(r) => (r.dst_x as i32, r.dst_y as i32),
                    Err(_) => self.fallback_position(win_height),
                }
            }
            None => self.fallback_position(win_height),
        };

        // Resize and reposition.
        let config = ConfigureWindowAux::new()
            .x(abs_x)
            .y(abs_y)
            .width(win_width as u32)
            .height(win_height as u32);
        self.conn.configure_window(self.win, &config)?;

        // Map the window if not already visible.
        if !self.mapped {
            self.conn.map_window(self.win)?;
            self.mapped = true;
        }

        // Clear window.
        self.conn
            .clear_area(true, self.win, 0, 0, win_width, win_height)?;

        let baseline_y = PADDING + self.font_ascent;

        if let Some(split) = ghost_char_idx {
            let (input_chars, ghost_chars) = all_chars.split_at(split);

            // Draw user-typed portion in normal colour.
            if !input_chars.is_empty() {
                self.conn
                    .image_text16(self.win, self.gc, PADDING, baseline_y, input_chars)?;
            }

            // Draw ghost portion in grey, offset by the width of the input part.
            if !ghost_chars.is_empty() {
                let input_width = if input_chars.is_empty() {
                    0i16
                } else {
                    self.conn
                        .query_text_extents(Fontable::from(self.gc), input_chars)?
                        .reply()
                        .context("query_text_extents (input part)")?
                        .overall_width as i16
                };
                self.conn.image_text16(
                    self.win,
                    self.ghost_gc,
                    PADDING + input_width,
                    baseline_y,
                    ghost_chars,
                )?;
            }
        } else {
            // No ghost: draw everything in normal colour.
            self.conn
                .image_text16(self.win, self.gc, PADDING, baseline_y, &all_chars)?;
        }

        // Raise to top.
        let raise = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        self.conn.configure_window(self.win, &raise)?;
        self.conn.flush()?;

        Ok(())
    }

    /// Hides the preedit window.
    pub fn hide(&mut self) -> Result<()> {
        if self.mapped {
            self.conn.unmap_window(self.win)?;
            self.conn.flush()?;
            self.mapped = false;
        }
        Ok(())
    }

    /// Returns the root window that this preedit window lives on.
    fn root_window(&self) -> Result<Window> {
        let geom = self.conn.get_geometry(self.win)?.reply()?;
        Ok(geom.root)
    }

    /// Fallback position: bottom-left of the screen.
    fn fallback_position(&self, win_height: u16) -> (i32, i32) {
        (
            0,
            self.screen_height
                .saturating_sub(win_height + BORDER_WIDTH * 2) as i32,
        )
    }
}

impl Drop for PreeditWindow {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.free_gc(self.gc);
        let _ = self.conn.free_gc(self.ghost_gc);
    }
}
