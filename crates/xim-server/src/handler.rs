//! XIM protocol handler implementing the `xim::ServerHandler` trait.
//!
//! Each input context (IC) created by a client gets its own `SessionId` from
//! `skk-daemon` via D-Bus.  Key events are forwarded to the daemon and the
//! resulting `IpcAction`s are dispatched back through the XIM protocol
//! (commit, preedit, forward).

use tracing::{debug, error, info, warn};
use x11rb::protocol::xproto::KeyPressEvent;
use xim::{InputStyle, Server, ServerError, ServerHandler, UserInputContext};

use skk_ipc::dispatch::{dispatch as dispatch_actions, ActionSink};
use skk_ipc::proxy::reconnect::{LocalHandle, ReconnectingClient};

use crate::candidates::CandidateWindow;
use crate::key::KeyMap;
use crate::preedit::{PreeditWindow, SpotHint};

/// Per-IC data stored inside the xim crate's `UserInputContext`.
pub struct IcData {
    /// Local handle into [`Handler::client`]; maps to a daemon session internally.
    pub handle: LocalHandle,
}

/// XIM server handler that bridges XIM events to skk-daemon via D-Bus.
pub struct Handler {
    client: ReconnectingClient,
    keymap: KeyMap,
    preedit: PreeditWindow,
    candidates: CandidateWindow,
}

impl Handler {
    pub fn new(
        client: ReconnectingClient,
        keymap: KeyMap,
        preedit: PreeditWindow,
        candidates: CandidateWindow,
    ) -> Self {
        Self {
            client,
            keymap,
            preedit,
            candidates,
        }
    }
}

// ── ActionSink adapter for a single XIM input context ────────────────────────

/// Borrows the XIM server and IC for the duration of one key dispatch.
struct IcSink<'a, S: Server<XEvent = KeyPressEvent>> {
    server: &'a mut S,
    user_ic: &'a mut UserInputContext<IcData>,
    preedit: &'a mut PreeditWindow,
    candidates: &'a mut CandidateWindow,
    spot: Option<SpotHint>,
    handle: LocalHandle,
}

impl<S: Server<XEvent = KeyPressEvent>> ActionSink for IcSink<'_, S> {
    fn commit(&mut self, text: &str) {
        if let Err(e) = self.preedit.hide() {
            warn!(handle = self.handle, "preedit hide failed: {e}");
        }
        if let Err(e) = self.server.commit(&self.user_ic.ic, text) {
            warn!(handle = self.handle, "XIM commit failed: {e}");
        }
    }

    fn update_preedit(&mut self, text: &str, _cursor: u32, ghost_start: Option<u32>) {
        let ghost = ghost_start.map(|g| g as usize);
        if let Err(e) = self.preedit.update(text, ghost, self.spot.as_ref()) {
            warn!(handle = self.handle, "preedit update failed: {e}");
        }
    }

    fn clear_preedit(&mut self) {
        if let Err(e) = self.preedit.hide() {
            warn!(handle = self.handle, "preedit hide failed: {e}");
        }
    }

    fn show_candidates(&mut self, candidates: &[String], focused: u32, sel_keys: &str) {
        if let Err(e) = self
            .candidates
            .show(candidates, sel_keys, focused, self.spot.as_ref())
        {
            warn!(handle = self.handle, "candidate show failed: {e}");
        }
    }

    fn hide_candidates(&mut self) {
        if let Err(e) = self.candidates.hide() {
            warn!(handle = self.handle, "candidate hide failed: {e}");
        }
    }

    fn update_status(&mut self, indicator: &str, _timeout_ms: u32) {
        debug!(handle = self.handle, status = indicator, "UpdateStatus");
    }
}

// ── ServerHandler implementation ──────────────────────────────────────────────

impl<S: Server<XEvent = KeyPressEvent>> ServerHandler<S> for Handler {
    type InputContextData = IcData;
    type InputStyleArray = [InputStyle; 3];

    fn new_ic_data(
        &mut self,
        _server: &mut S,
        _style: InputStyle,
    ) -> Result<Self::InputContextData, ServerError> {
        let handle = self.client.create_handle("xim");
        info!(handle, "created local handle for new IC");
        Ok(IcData { handle })
    }

    fn input_styles(&self) -> Self::InputStyleArray {
        [
            // On-the-spot (preedit callbacks — client draws preedit).
            InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
            // Over-the-spot (server draws preedit at spot location).
            InputStyle::PREEDIT_POSITION | InputStyle::STATUS_NOTHING,
            // Root-window / no preedit.
            InputStyle::PREEDIT_NOTHING | InputStyle::STATUS_NOTHING,
        ]
    }

    fn filter_events(&self) -> u32 {
        // KeyPressMask (bit 0) — we want key press events forwarded to us.
        1
    }

    fn handle_connect(&mut self, _server: &mut S) -> Result<(), ServerError> {
        info!("XIM client connected");
        Ok(())
    }

    fn handle_create_ic(
        &mut self,
        server: &mut S,
        user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<(), ServerError> {
        debug!(
            handle = user_ic.user_data.handle,
            "IC created, setting event mask"
        );
        // Forward KeyPress (bit 0) events to the handler.
        server.set_event_mask(&user_ic.ic, 1, 0)
    }

    fn handle_destroy_ic(
        &mut self,
        _server: &mut S,
        user_ic: UserInputContext<Self::InputContextData>,
    ) -> Result<(), ServerError> {
        let sid = user_ic.user_data.handle;
        info!(handle = sid, "IC destroyed, releasing daemon session");
        self.client.destroy_handle(sid);
        Ok(())
    }

    fn handle_reset_ic(
        &mut self,
        _server: &mut S,
        _user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<String, ServerError> {
        // Return empty string — no pending commit text to flush.
        Ok(String::new())
    }

    fn handle_set_focus(
        &mut self,
        _server: &mut S,
        user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<(), ServerError> {
        debug!(handle = user_ic.user_data.handle, "IC gained focus");
        Ok(())
    }

    fn handle_unset_focus(
        &mut self,
        _server: &mut S,
        user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<(), ServerError> {
        debug!(handle = user_ic.user_data.handle, "IC lost focus");
        Ok(())
    }

    fn handle_set_ic_values(
        &mut self,
        _server: &mut S,
        _user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<(), ServerError> {
        // Accept whatever the client sends (spot location, area, etc.).
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        server: &mut S,
        user_ic: &mut UserInputContext<Self::InputContextData>,
        xev: &S::XEvent,
    ) -> Result<bool, ServerError> {
        let sid = user_ic.user_data.handle;

        // response_type 2 = KeyPress, 3 = KeyRelease.
        let is_press = xev.response_type & !0x80 == 2;

        let (keysym, modifiers) = self.keymap.resolve(xev.detail, xev.state.into());

        // Skip NoSymbol.
        if keysym == 0 {
            return Ok(false);
        }

        let actions = match self.client.process_key(sid, keysym, modifiers, is_press) {
            Ok(a) => a,
            Err(e) => {
                error!(handle = sid, "process_key error: {:?}", e);
                return Ok(false);
            }
        };

        let spot = {
            let spot = user_ic.ic.preedit_spot();
            let focus_win = user_ic
                .ic
                .app_focus_win()
                .or(user_ic.ic.app_win())
                .map(|w| w.get());
            focus_win.map(|fw| SpotHint {
                x: spot.x,
                y: spot.y,
                focus_win: fw,
            })
        };

        let mut sink = IcSink {
            server,
            user_ic,
            preedit: &mut self.preedit,
            candidates: &mut self.candidates,
            spot,
            handle: sid,
        };
        let result = dispatch_actions(&actions, &mut sink);
        Ok(result.consumed && !result.force_passthrough)
    }
}
