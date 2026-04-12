//! XIM protocol handler implementing the `xim::ServerHandler` trait.
//!
//! Each input context (IC) gets its own `SessionId` from the daemon's
//! `SessionManager` directly (no D-Bus round-trip).  Key events are
//! processed in-process and the resulting `IpcAction`s are dispatched
//! back through the XIM protocol (commit, preedit, forward).

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use x11rb::protocol::xproto::KeyPressEvent;
use xim::{InputStyle, Server, ServerError, ServerHandler, UserInputContext};

use skk_ipc::{
    SessionId, ACTION_CLEAR_PREEDIT, ACTION_COMMIT, ACTION_HIDE_CANDIDATES, ACTION_PASSTHROUGH,
    ACTION_SHOW_CANDIDATES, ACTION_UPDATE_PREEDIT, ACTION_UPDATE_STATUS,
};

use crate::session::SessionManager;

use super::candidates::CandidateWindow;
use super::key::KeyMap;
use super::preedit::{PreeditWindow, SpotHint};

/// Per-IC data stored inside the xim crate's `UserInputContext`.
pub struct IcData {
    pub session_id: SessionId,
}

/// XIM server handler that bridges XIM events to the in-process SessionManager.
pub struct Handler {
    sessions: Arc<Mutex<SessionManager>>,
    keymap: KeyMap,
    preedit: PreeditWindow,
    candidates: CandidateWindow,
}

impl Handler {
    pub fn new(
        sessions: Arc<Mutex<SessionManager>>,
        keymap: KeyMap,
        preedit: PreeditWindow,
        candidates: CandidateWindow,
    ) -> Self {
        Self {
            sessions,
            keymap,
            preedit,
            candidates,
        }
    }
}

impl<S: Server<XEvent = KeyPressEvent>> ServerHandler<S> for Handler {
    type InputContextData = IcData;
    type InputStyleArray = [InputStyle; 3];

    fn new_ic_data(
        &mut self,
        _server: &mut S,
        _style: InputStyle,
    ) -> Result<Self::InputContextData, ServerError> {
        let session_id = self.sessions.blocking_lock().create_session("xim");
        info!(session_id, "created daemon session for new IC");
        Ok(IcData { session_id })
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
            session_id = user_ic.user_data.session_id,
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
        let sid = user_ic.user_data.session_id;
        info!(session_id = sid, "IC destroyed, releasing daemon session");
        self.sessions.blocking_lock().destroy_session(sid);
        Ok(())
    }

    fn handle_reset_ic(
        &mut self,
        _server: &mut S,
        _user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<String, ServerError> {
        Ok(String::new())
    }

    fn handle_set_focus(
        &mut self,
        _server: &mut S,
        user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<(), ServerError> {
        debug!(
            session_id = user_ic.user_data.session_id,
            "IC gained focus"
        );
        Ok(())
    }

    fn handle_unset_focus(
        &mut self,
        _server: &mut S,
        user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<(), ServerError> {
        debug!(
            session_id = user_ic.user_data.session_id,
            "IC lost focus"
        );
        Ok(())
    }

    fn handle_set_ic_values(
        &mut self,
        _server: &mut S,
        _user_ic: &mut UserInputContext<Self::InputContextData>,
    ) -> Result<(), ServerError> {
        // Accept whatever the client sends (spot location, area, etc.).
        // Phase 2 will read spot location for over-the-spot preedit.
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        server: &mut S,
        user_ic: &mut UserInputContext<Self::InputContextData>,
        xev: &S::XEvent,
    ) -> Result<bool, ServerError> {
        let sid = user_ic.user_data.session_id;

        // response_type 2 = KeyPress, 3 = KeyRelease.
        let is_press = xev.response_type & !0x80 == 2;

        let (keysym, modifiers) = self.keymap.resolve(xev.detail, xev.state.into());

        // Skip NoSymbol.
        if keysym == 0 {
            return Ok(false);
        }

        let actions = self
            .sessions
            .blocking_lock()
            .process_key(sid, keysym, modifiers, is_press);

        // Build spot hint from the IC's preedit_spot and focus/client window.
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

        // Dispatch actions — mirrors adapter-gtk3 lines 140–194.
        let mut consumed = false;
        let mut force_passthrough = false;

        for action in &actions {
            match action.kind {
                k if k == ACTION_PASSTHROUGH => {
                    force_passthrough = true;
                }
                k if k == ACTION_COMMIT => {
                    consumed = true;
                    // Hide preedit before committing.
                    if let Err(e) = self.preedit.hide() {
                        warn!(session_id = sid, "preedit hide failed: {e}");
                    }
                    if let Err(e) = server.commit(&user_ic.ic, &action.text) {
                        warn!(session_id = sid, "XIM commit failed: {e}");
                    }
                }
                k if k == ACTION_UPDATE_PREEDIT => {
                    consumed = true;
                    if let Err(e) = self.preedit.update(&action.text, spot.as_ref()) {
                        warn!(session_id = sid, "preedit update failed: {e}");
                    }
                }
                k if k == ACTION_CLEAR_PREEDIT => {
                    consumed = true;
                    if let Err(e) = self.preedit.hide() {
                        warn!(session_id = sid, "preedit hide failed: {e}");
                    }
                }
                k if k == ACTION_SHOW_CANDIDATES => {
                    consumed = true;
                    if let Err(e) = self.candidates.show(
                        &action.candidates,
                        &action.text, // selection keys
                        action.focused,
                        spot.as_ref(),
                    ) {
                        warn!(session_id = sid, "candidate show failed: {e}");
                    }
                }
                k if k == ACTION_HIDE_CANDIDATES => {
                    consumed = true;
                    if let Err(e) = self.candidates.hide() {
                        warn!(session_id = sid, "candidate hide failed: {e}");
                    }
                }
                k if k == ACTION_UPDATE_STATUS => {
                    // Phase 3: indicator update.
                    debug!(session_id = sid, status = action.text, "UpdateStatus");
                }
                _ => {}
            }
        }

        Ok(consumed && !force_passthrough)
    }
}
