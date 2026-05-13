use std::collections::HashSet;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use wayland_client::{
    protocol::{
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_output::WlOutput,
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
        wl_shm::WlShm,
    },
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::input_method::zv1::client::{
    zwp_input_method_context_v1::{self, ZwpInputMethodContextV1},
    zwp_input_method_v1::{self, ZwpInputMethodV1},
    zwp_input_panel_v1::ZwpInputPanelV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

use skk_ipc::dispatch::{dispatch as dispatch_actions, ActionSink};
use skk_ipc::proxy::reconnect::{LocalHandle, ReconnectingClient};

use crate::candidate_window::{self, CandidateWindow};
use crate::keymap;

// ── Timer-pipe byte tags ──────────────────────────────────────────────────────

/// Byte written to the timer pipe when the status auto-hide timer fires.
const TAG_STATUS_HIDE: u8 = 1;
/// Byte written to the timer pipe for each key-repeat tick.
const TAG_KEY_REPEAT: u8 = 2;
/// Byte written to the timer pipe by the heartbeat thread roughly once per
/// minute. The main loop logs a short summary of its IME state on receipt so
/// long passthrough periods can be distinguished from a stuck event loop.
const TAG_HEARTBEAT: u8 = 3;
/// Interval between heartbeat ticks.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Marker user-data for the wl_keyboard obtained via wl_seat.get_keyboard.
/// Gives it a distinct Dispatch impl from the IM-grabbed keyboard (which
/// uses `()` as its user-data).  We only care about RepeatInfo on this one.
#[derive(Debug)]
pub struct SeatKbd;

/// State tracking one currently-repeating key.
struct RepeatActive {
    keycode: u32,
    keysym: u32,
    /// True if the IM consumed the initial press.  When false the repeat
    /// cycles release+press via `context.key()` so the app treats each tick
    /// as a fresh keypress (apps usually ignore duplicate press events).
    consumed: bool,
    cancel: Arc<AtomicBool>,
}

/// Returns true if this keysym should participate in auto-repeat.
/// Excludes modifier keys, Caps/Num/Scroll Lock, and XKB_KEY_NoSymbol.
fn keysym_repeats(keysym: u32) -> bool {
    if keysym == 0 {
        return false;
    }
    // Modifier keysyms: 0xFFE1 - 0xFFEE  (Shift/Ctrl/Meta/Alt/Super/Hyper L+R).
    // Lock keys: Caps_Lock 0xFFE5, Shift_Lock 0xFFE6, Num_Lock 0xFF7F, Scroll_Lock 0xFF14.
    if (0xFFE1..=0xFFEE).contains(&keysym) {
        return false;
    }
    if matches!(keysym, 0xFF7F | 0xFF14) {
        return false;
    }
    true
}

// ── Per-activation state ──────────────────────────────────────────────────────

struct ActiveContext {
    context: ZwpInputMethodContextV1,
    /// The IM-grabbed wl_keyboard. Kept alive (no longer explicitly released)
    /// for the lifetime of this ActiveContext so wayland-client keeps routing
    /// Modifiers/Key events to our Dispatch impl. KWin invalidates the
    /// underlying object server-side when Deactivate fires, so the proxy is
    /// just dropped at that point.
    _keyboard: WlKeyboard,
    serial: u32,
    skk_mods: u32,
    xkb_state: Option<keymap::XkbState>,
}

// ── Top-level state ───────────────────────────────────────────────────────────

pub struct WaylandState {
    client: ReconnectingClient,
    /// SKK session handle. Created once at startup and reused across every
    /// Activate/Deactivate cycle so the SKK state (kana mode, ▽/▼ mode,
    /// candidate index, …) survives the rapid Activate/Deactivate churn
    /// that some compositors (e.g. KWin with Electron apps) emit on every
    /// key press. Without this, each cycle would create a fresh session
    /// and the user would be kicked back to the default hiragana mode.
    handle: LocalHandle,
    input_method: Option<ZwpInputMethodV1>,
    active: Option<ActiveContext>,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    /// `zwp_input_panel_v1` global, when the compositor advertises it.
    /// When present, candidate window and status indicator surfaces are
    /// hosted as input panel overlays so the compositor places them near
    /// the application's text-input cursor. When absent, both fall back
    /// to `zwlr_layer_shell_v1` anchored at the screen corner.
    pub input_panel: Option<ZwpInputPanelV1>,
    outputs: Vec<WlOutput>,
    pub candidate_windows: Vec<CandidateWindow>,
    qh: Option<QueueHandle<WaylandState>>,
    /// Write end of the timer pipe.  Used for both status auto-hide and key
    /// repeat; the byte value distinguishes them (see TAG_* constants).
    timer_write: OwnedFd,
    /// Cancel flag for the in-flight status timer thread.
    status_cancel: Option<Arc<AtomicBool>>,
    /// Keyboard repeat rate (keys/sec) reported by wl_keyboard.repeat_info.
    /// 0 disables repeat entirely.
    repeat_rate: u32,
    /// Initial delay in ms before key repeat starts.
    repeat_delay: u32,
    /// Currently-repeating key, if any.
    repeat_active: Option<RepeatActive>,
    /// Evdev keycodes currently believed to be pressed.  Used to filter out
    /// stray release events that KWin appears to synthesize when it processes
    /// input-method commits (these show up as a release-only cadence after
    /// commit_string calls).
    pressed_keys: HashSet<u32>,
    /// Deadline for stopping the current repeat.  When KWin sends a release
    /// event for the repeating key we don't immediately stop — the event is
    /// likely a synthetic artifact of a commit_string.  Each subsequent
    /// release event for the same key pushes the deadline further; the
    /// repeat actually stops once this deadline elapses with no new events.
    pending_stop_release: Option<Instant>,
    /// wl_seat bound from the registry, used to get a regular wl_keyboard
    /// for standard repeat_info delivery (KWin does not emit repeat_info on
    /// the IM-grabbed keyboard).
    seat: Option<WlSeat>,
    /// Regular wl_keyboard obtained via wl_seat.get_keyboard.  Kept alive so
    /// compositor-driven RepeatInfo updates keep arriving for the lifetime
    /// of the process.  We ignore all its other events.
    _seat_keyboard: Option<WlKeyboard>,
}

impl WaylandState {
    fn new(timer_write: OwnedFd) -> anyhow::Result<Self> {
        let client = ReconnectingClient::new()?;
        let handle = client.create_handle("wayland");
        tracing::info!(
            handle,
            "created SKK session handle (persistent across activations)"
        );
        Ok(Self {
            client,
            handle,
            input_method: None,
            active: None,
            compositor: None,
            shm: None,
            layer_shell: None,
            input_panel: None,
            outputs: Vec::new(),
            candidate_windows: Vec::new(),
            qh: None,
            timer_write,
            status_cancel: None,
            // Sensible defaults until repeat_info arrives.
            repeat_rate: 25,
            repeat_delay: 500,
            repeat_active: None,
            pressed_keys: HashSet::new(),
            pending_stop_release: None,
            seat: None,
            _seat_keyboard: None,
        })
    }

    /// How long to wait after a release event before actually stopping a
    /// repeat.  KWin emits synthetic release events in bursts after each
    /// commit_string; we must wait long enough for the next burst (caused
    /// by our next repeat tick) to reset the deadline.  The grace therefore
    /// scales with the tick interval.
    fn repeat_stop_grace(&self) -> Duration {
        let interval_ms = 1000u64 / (self.repeat_rate.max(1) as u64);
        // 2× the tick interval gives ample overlap between consecutive
        // synthetic-release bursts at low rates; clamp to a sane range.
        let ms = (interval_ms * 2).clamp(60, 500);
        Duration::from_millis(ms)
    }

    /// Begin repeating the given key after `repeat_delay` ms, then at
    /// `repeat_rate` presses per second.  Cancels any previous repeat.
    fn start_repeat(&mut self, keycode: u32, keysym: u32, consumed: bool) {
        if let Some(old) = self.repeat_active.take() {
            old.cancel.store(true, Ordering::Relaxed);
            self.pressed_keys.remove(&old.keycode);
            // If the old repeat was passthrough, send a final release so the
            // app doesn't end up thinking the old key is still pressed.
            if !old.consumed {
                self.send_key_release_to_app(old.keycode);
            }
        }
        self.pending_stop_release = None;
        if self.repeat_rate == 0 {
            tracing::debug!("start_repeat: disabled (rate=0)");
            return;
        }
        let fd = match self.timer_write.as_fd().try_clone_to_owned() {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("repeat fd clone failed: {e}");
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.repeat_active = Some(RepeatActive {
            keycode,
            keysym,
            consumed,
            cancel: cancel.clone(),
        });

        let delay_ms = self.repeat_delay as u64;
        let interval_ms = (1000u32 / self.repeat_rate.max(1)).max(1) as u64;

        tracing::debug!(
            "start_repeat: code={keycode}, sym={keysym:#x}, delay={delay_ms}ms, interval={interval_ms}ms"
        );

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(delay_ms));
            let mut ticks = 0u32;
            while !cancel.load(Ordering::Relaxed) {
                match rustix::io::write(&fd, &[TAG_KEY_REPEAT]) {
                    Ok(_) => {
                        ticks += 1;
                    }
                    Err(e) => {
                        tracing::debug!("repeat thread exit on write error: {e:?}");
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(interval_ms));
            }
            tracing::debug!("repeat thread exit after {ticks} ticks");
        });
    }

    /// Cancel any in-flight repeat unconditionally.  Used on deactivate and
    /// whenever the modifier state changes (so Shift+1 stops repeating when
    /// Shift is released, matching native Wayland behaviour).
    fn stop_all_repeat(&mut self) {
        if let Some(ra) = self.repeat_active.take() {
            ra.cancel.store(true, Ordering::Relaxed);
            self.pressed_keys.remove(&ra.keycode);
            if !ra.consumed {
                self.send_key_release_to_app(ra.keycode);
            }
        }
        self.pending_stop_release = None;
    }

    /// Directly send a release event for `keycode` to the focused app (no SKK
    /// processing).  Used for passthrough repeat handling.
    fn send_key_release_to_app(&self, keycode: u32) {
        if let Some(active) = self.active.as_ref() {
            active.context.key(active.serial, 0, keycode, 0);
        }
    }

    /// Directly send a press event for `keycode` to the focused app.  Used
    /// for passthrough repeat handling (each tick is a release+press pair).
    fn send_key_press_to_app(&self, keycode: u32) {
        if let Some(active) = self.active.as_ref() {
            active.context.key(active.serial, 0, keycode, 1);
        }
    }

    /// Create one CandidateWindow per known output. Called once after the
    /// first roundtrip when all globals (including outputs) have been
    /// enumerated.
    fn init_candidate_windows(&mut self) {
        let (Some(compositor), Some(shm), Some(qh)) = (
            self.compositor.as_ref(),
            self.shm.as_ref(),
            self.qh.as_ref(),
        ) else {
            tracing::warn!("missing Wayland globals — candidate window disabled");
            return;
        };

        if self.input_panel.is_none() && self.layer_shell.is_none() {
            tracing::warn!(
                "neither zwp_input_panel_v1 nor zwlr_layer_shell_v1 advertised — candidate window disabled"
            );
            return;
        }

        let Some(font) = candidate_window::load_font() else {
            return;
        };
        let font = Arc::new(font);

        let outputs: Vec<Option<WlOutput>> = if self.outputs.is_empty() {
            tracing::warn!("no wl_output found — creating one candidate window (default output)");
            vec![None]
        } else {
            self.outputs.iter().map(|o| Some(o.clone())).collect()
        };

        for output_opt in outputs {
            if let Some(cw) = CandidateWindow::new(
                compositor,
                self.layer_shell.as_ref(),
                self.input_panel.as_ref(),
                shm.clone(),
                Arc::clone(&font),
                qh,
                output_opt.as_ref(),
            ) {
                self.candidate_windows.push(cw);
            }
        }

        tracing::info!(
            "created {} candidate window(s)",
            self.candidate_windows.len()
        );
    }

    /// Returns true if the IM consumed the key (no forwarding happened).
    /// Returns false for passthrough keys — these are forwarded to the app via
    /// `context.key()` and the app is responsible for its own key repeat.
    ///
    /// `key_serial` / `key_time` are the serial and time from the original
    /// `wl_keyboard.key` event. They are forwarded to `context.key` so KWin
    /// can correlate the forwarded key with the original keyboard event — using
    /// the input-method context's commit serial (`active.serial`) here was
    /// observed to cause KWin to silently drop forwarded keys for some apps
    /// (e.g. Electron's Enter/Backspace in Slack and X).
    fn on_key(
        &mut self,
        key_serial: u32,
        key_time: u32,
        keycode: u32,
        keysym: u32,
        is_press: bool,
    ) -> bool {
        let (commit_serial, skk_mods) = match self.active.as_ref() {
            Some(a) => (a.serial, a.skk_mods),
            None => {
                // Visible at the default info level: this almost certainly
                // means the IME thinks it is inactive while keys are still
                // being delivered to its grabbed wl_keyboard (state desync).
                tracing::warn!(
                    "on_key: no active context, dropping event (code={keycode}, sym={keysym:#x})"
                );
                return false;
            }
        };

        let actions = match self
            .client
            .process_key(self.handle, keysym, skk_mods, is_press)
        {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(handle = self.handle, "process_key error: {e:?}");
                return false;
            }
        };
        tracing::debug!(
            "on_key(code={keycode}, sym={keysym:#x}, press={is_press}) → {} action(s) (commit_serial={commit_serial})",
            actions.len()
        );

        if self.active.is_none() {
            return false;
        }

        let context = &self.active.as_ref().unwrap().context;
        let qh = self.qh.as_ref();
        let cws = &mut self.candidate_windows;
        let timer_fd = self.timer_write.as_fd();
        let cancel = &mut self.status_cancel;

        let mut sink = ContextSink {
            context,
            serial: commit_serial,
            candidate_windows: cws,
            qh,
            timer_write_fd: timer_fd,
            status_cancel: cancel,
        };
        let result = dispatch_actions(&actions, &mut sink);

        let consumed = result.consumed && !result.force_passthrough;
        if !consumed {
            let state_val: u32 = if is_press { 1 } else { 0 };
            self.active
                .as_ref()
                .unwrap()
                .context
                .key(key_serial, key_time, keycode, state_val);
        }
        consumed
    }
}

// ── ActionSink ────────────────────────────────────────────────────────────────

struct ContextSink<'a> {
    context: &'a ZwpInputMethodContextV1,
    serial: u32,
    candidate_windows: &'a mut Vec<CandidateWindow>,
    qh: Option<&'a QueueHandle<WaylandState>>,
    timer_write_fd: BorrowedFd<'a>,
    status_cancel: &'a mut Option<Arc<AtomicBool>>,
}

impl ActionSink for ContextSink<'_> {
    fn commit(&mut self, text: &str) {
        tracing::debug!("commit({text:?}) with serial={}", self.serial);
        self.context.commit_string(self.serial, text.to_string());
    }

    fn update_preedit(&mut self, text: &str, cursor: u32, _ghost_start: Option<u32>) {
        self.context.preedit_cursor(cursor as i32);
        self.context
            .preedit_string(self.serial, text.to_string(), String::new());
    }

    fn clear_preedit(&mut self) {
        self.context
            .preedit_string(self.serial, String::new(), String::new());
    }

    fn show_candidates(&mut self, candidates: &[String], focused: u32, sel_keys: &str) {
        if let Some(qh) = self.qh {
            for cw in self.candidate_windows.iter_mut() {
                cw.show(candidates, focused, sel_keys, qh);
            }
        }
    }

    fn hide_candidates(&mut self) {
        for cw in self.candidate_windows.iter_mut() {
            cw.hide();
        }
    }

    fn update_status(&mut self, indicator: &str, timeout_ms: u32) {
        // Cancel any previous auto-hide timer.
        if let Some(old) = self.status_cancel.take() {
            old.store(true, Ordering::Relaxed);
        }

        if let Some(qh) = self.qh {
            for cw in self.candidate_windows.iter_mut() {
                cw.show_status(indicator, qh);
            }
        }

        // Schedule auto-hide.
        if timeout_ms > 0 {
            let fd = match self.timer_write_fd.try_clone_to_owned() {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("status timer fd clone failed: {e}");
                    return;
                }
            };
            let cancel = Arc::new(AtomicBool::new(false));
            *self.status_cancel = Some(cancel.clone());

            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(timeout_ms as u64));
                if !cancel.load(Ordering::Relaxed) {
                    let _ = rustix::io::write(&fd, &[TAG_STATUS_HIDE]);
                }
            });
        }
    }
}

// ── Dispatch implementations ──────────────────────────────────────────────────

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                i if i == ZwpInputMethodV1::interface().name => {
                    tracing::info!("found zwp_input_method_v1 (version {version})");
                    state.input_method = Some(registry.bind(name, 1, qh, ()));
                }
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, 4.min(version), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, 1, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    tracing::info!("found zwlr_layer_shell_v1 (version {version})");
                    state.layer_shell = Some(registry.bind(name, 4.min(version), qh, ()));
                }
                i if i == ZwpInputPanelV1::interface().name => {
                    tracing::info!("found zwp_input_panel_v1 (version {version})");
                    state.input_panel = Some(registry.bind(name, 1, qh, ()));
                }
                "wl_output" => {
                    let output: WlOutput = registry.bind(name, 2.min(version), qh, ());
                    state.outputs.push(output);
                }
                "wl_seat" => {
                    // wl_seat.get_keyboard is called later when the seat
                    // reports a keyboard capability.
                    let seat: WlSeat = registry.bind(name, 7.min(version), qh, ());
                    state.seat = Some(seat);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwpInputMethodV1, ()> for WaylandState {
    fn event_created_child(
        _opcode: u16,
        qhandle: &QueueHandle<Self>,
    ) -> std::sync::Arc<dyn wayland_client::backend::ObjectData> {
        qhandle.make_data::<ZwpInputMethodContextV1, ()>(())
    }

    fn event(
        state: &mut Self,
        _im: &ZwpInputMethodV1,
        event: zwp_input_method_v1::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v1::Event::Activate { id } => {
                if let Some(prev) = state.active.take() {
                    tracing::warn!("new activation before previous deactivation — cleaning up");
                    // Do not destroy the SKK handle: it is owned by
                    // WaylandState and reused across every Activate cycle
                    // so the user's kana mode survives the focus churn that
                    // some compositors (KWin with Electron apps) cause.
                    // Same rationale as the Deactivate branch for the
                    // Wayland-side proxies: do not call
                    // prev.keyboard.release() or prev.context.destroy() —
                    // when a fresh Activate arrives without an intervening
                    // Deactivate, KWin has likely already invalidated the
                    // previous keyboard and context server-side, so sending
                    // the destructor would crash the wl_display connection.
                    drop(prev);
                }
                let keyboard = id.grab_keyboard(qh, ());
                tracing::info!(handle = state.handle, "input context activated");
                state.active = Some(ActiveContext {
                    context: id,
                    _keyboard: keyboard,
                    serial: 0,
                    skk_mods: 0,
                    xkb_state: None,
                });
            }
            zwp_input_method_v1::Event::Deactivate { context: _ctx } => {
                state.stop_all_repeat();
                // Do NOT clear pressed_keys here. KWin's rapid Activate/Deactivate
                // (e.g. Electron apps' focus churn on each Enter) can carry a
                // key press across two grabs — press arrives on the old grab,
                // release arrives on the new one. If we clear here, the
                // release event hits the `stray release` branch in the Key
                // handler and gets dropped, leaving the app stuck thinking
                // the key is still held (Slack/X "Enter only works half the
                // time" pattern). The synthetic-release filter still works:
                // any release for a key that was never pressed will not be
                // in pressed_keys and gets dropped as before.
                if let Some(active) = state.active.take() {
                    tracing::info!(handle = state.handle, "input context deactivated");
                    // Do not destroy the SKK handle: see Activate branch above.
                    // Do NOT call active.keyboard.release() or active.context.destroy():
                    // KWin invalidates both the grabbed wl_keyboard and the
                    // zwp_input_method_context_v1 server-side when it sends Deactivate.
                    // Sending the destructor request hits a dead object and crashes the
                    // whole wl_display connection with "invalid object" (observed
                    // 2026-05-14 under rapid Activate/Deactivate from Slack focus
                    // churn). Dropping the proxies without sending the destructor is
                    // safe — the wayland-client crate does not auto-send destructors
                    // on Drop.
                    drop(active);
                }
            }
            _ => {}
        }
    }
}

// `zwp_input_panel_v1` has no events, only requests.
impl Dispatch<ZwpInputPanelV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpInputPanelV1,
        _event: <ZwpInputPanelV1 as Proxy>::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpInputMethodContextV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _ctx: &ZwpInputMethodContextV1,
        event: zwp_input_method_context_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_context_v1::Event::CommitState { serial } => {
                if let Some(active) = &mut state.active {
                    active.serial = serial;
                }
            }
            zwp_input_method_context_v1::Event::Reset => {
                if let Some(active) = state.active.as_ref() {
                    active
                        .context
                        .preedit_string(active.serial, String::new(), String::new());
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _kb: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    tracing::warn!("unsupported keymap format");
                    return;
                }
                match keymap::XkbState::from_fd(fd, size) {
                    Ok(xkb) => {
                        if let Some(active) = &mut state.active {
                            active.xkb_state = Some(xkb);
                        }
                    }
                    Err(e) => tracing::error!("failed to load XKB keymap: {e}"),
                }
            }
            wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: key_state,
            } => {
                let is_press = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));

                if is_press {
                    if !state.pressed_keys.insert(key) {
                        // The key was already in pressed_keys.  When the user
                        // releases a repeating key we defer `stop_repeat` (to
                        // ride out KWin's synthetic-release stream), leaving
                        // the key still marked pressed.  A press that arrives
                        // during that grace window is a rapid re-tap of the
                        // same key — close out the old repeat cleanly and
                        // treat the event as a fresh press.
                        let is_repeating =
                            state.repeat_active.as_ref().map(|r| r.keycode) == Some(key);
                        if is_repeating && state.pending_stop_release.is_some() {
                            tracing::debug!("rapid re-tap of {key} during repeat grace");
                            state.stop_all_repeat();
                            state.pressed_keys.insert(key);
                        } else {
                            tracing::debug!("stray press for already-pressed key {key}; ignored");
                            return;
                        }
                    }
                } else {
                    // Release for a key we are actively repeating: don't stop
                    // immediately.  KWin emits synthetic release events at the
                    // repeat cadence after commit_string; only a pause in the
                    // release stream indicates a real user release.
                    let is_repeating = state.repeat_active.as_ref().map(|r| r.keycode) == Some(key);
                    if is_repeating {
                        let grace = state.repeat_stop_grace();
                        state.pending_stop_release = Some(Instant::now() + grace);
                        tracing::debug!(
                            "release for repeating key {key}: defer stop (grace {} ms)",
                            grace.as_millis()
                        );
                        return;
                    }
                    if !state.pressed_keys.remove(&key) {
                        tracing::debug!("stray release for non-pressed key {key}; ignored");
                        return;
                    }
                }

                let keysym = state
                    .active
                    .as_ref()
                    .and_then(|a| a.xkb_state.as_ref())
                    .map(|s| s.key_get_one_sym(key))
                    .unwrap_or(0);
                let xkb_allows_repeat = state
                    .active
                    .as_ref()
                    .and_then(|a| a.xkb_state.as_ref())
                    .map(|s| s.key_repeats(key))
                    .unwrap_or(false);
                let repeat_eligible = xkb_allows_repeat || keysym_repeats(keysym);
                let consumed = if keysym != 0 {
                    state.on_key(serial, time, key, keysym, is_press)
                } else {
                    false
                };
                tracing::debug!(
                    "key: code={key}, sym={keysym:#x}, press={is_press}, consumed={consumed}, repeat_eligible={repeat_eligible}"
                );
                if is_press && repeat_eligible {
                    state.start_repeat(key, keysym, consumed);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.repeat_rate = rate.max(0) as u32;
                state.repeat_delay = delay.max(0) as u32;
                tracing::info!(
                    "key repeat info: rate={}/s, delay={}ms",
                    state.repeat_rate,
                    state.repeat_delay,
                );
            }
            wl_keyboard::Event::Modifiers {
                serial: _,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                let new_skk_mods = keymap::xkb_mods_to_skk(mods_depressed, mods_latched);
                let old_skk_mods = state.active.as_ref().map(|a| a.skk_mods).unwrap_or(0);
                // Visible at info level so we can confirm that the IM-grabbed
                // wl_keyboard is still receiving events when the passthrough
                // issue is reproduced.
                tracing::info!(
                    "modifiers event: dep={mods_depressed:#x} lat={mods_latched:#x} \
                     lock={mods_locked:#x} group={group} active={}",
                    state.active.is_some()
                );
                if let Some(active) = &mut state.active {
                    active.skk_mods = new_skk_mods;
                    if let Some(xkb) = &mut active.xkb_state {
                        xkb.update_mask(mods_depressed, mods_latched, mods_locked, group);
                    }
                }
                // Modifier change while a repeat is in flight: stop the
                // repeat so (e.g.) releasing Shift while holding '1' doesn't
                // morph `!` into `1` and keep repeating.
                if new_skk_mods != old_skk_mods && state.repeat_active.is_some() {
                    tracing::debug!("modifier changed during repeat; stopping");
                    state.stop_all_repeat();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Keyboard) && state._seat_keyboard.is_none() {
                tracing::debug!("wl_seat keyboard capability present; binding keyboard");
                state._seat_keyboard = Some(seat.get_keyboard(qh, SeatKbd));
            }
        }
    }
}

impl Dispatch<WlKeyboard, SeatKbd> for WaylandState {
    fn event(
        state: &mut Self,
        _kbd: &WlKeyboard,
        event: wl_keyboard::Event,
        _data: &SeatKbd,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::RepeatInfo { rate, delay } = event {
            state.repeat_rate = rate.max(0) as u32;
            state.repeat_delay = delay.max(0) as u32;
            tracing::info!(
                "key repeat info (seat): rate={}/s, delay={}ms",
                state.repeat_rate,
                state.repeat_delay,
            );
        }
        // Other events (Keymap, Enter/Leave, Key, Modifiers) are handled via
        // the IM-grabbed keyboard; ignore them here.
    }
}

// ── Event loop ────────────────────────────────────────────────────────────────

pub fn run() -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()
        .map_err(|e| anyhow::anyhow!("Wayland connection failed: {e}"))?;

    let display = conn.display();
    let mut event_queue: EventQueue<WaylandState> = conn.new_event_queue();
    let qh = event_queue.handle();

    // Create the status auto-hide timer pipe (non-blocking so drain loops don't block).
    let (timer_read, timer_write) = rustix::pipe::pipe_with(
        rustix::pipe::PipeFlags::NONBLOCK | rustix::pipe::PipeFlags::CLOEXEC,
    )?;

    // Heartbeat: a dedicated thread writes TAG_HEARTBEAT to the timer pipe
    // every HEARTBEAT_INTERVAL so the main loop emits a periodic state log.
    // This makes "main loop alive but no IME events arriving" distinguishable
    // from "main loop wedged" in the journalctl trace.
    {
        let fd = timer_write.as_fd().try_clone_to_owned()?;
        std::thread::Builder::new()
            .name("y2skk-heartbeat".into())
            .spawn(move || loop {
                std::thread::sleep(HEARTBEAT_INTERVAL);
                if rustix::io::write(&fd, &[TAG_HEARTBEAT]).is_err() {
                    // Main process is gone; the pipe write end can only fail
                    // when the reader has been closed. Bail out of the thread.
                    break;
                }
            })?;
    }

    let mut state = WaylandState::new(timer_write)?;
    state.qh = Some(qh.clone());

    display.get_registry(&qh, ());
    event_queue.roundtrip(&mut state)?;

    if state.input_method.is_none() {
        anyhow::bail!(
            "Compositor does not advertise zwp_input_method_v1. \
             Make sure you are running a Wayland compositor that supports it \
             (KDE Plasma 5/6 should work)."
        );
    }

    state.init_candidate_windows();
    if !state.candidate_windows.is_empty() {
        event_queue.roundtrip(&mut state)?;
        tracing::info!("candidate windows ready");
    }

    tracing::info!("y2skk-wayland ready");

    // Main loop: poll both the Wayland socket and the status timer pipe.
    use rustix::event::{poll, PollFd, PollFlags, Timespec};

    loop {
        // If a repeat-stop is pending and its grace period has elapsed, act.
        if let Some(deadline) = state.pending_stop_release {
            if Instant::now() >= deadline {
                if let Some(ra) = state.repeat_active.take() {
                    ra.cancel.store(true, Ordering::Relaxed);
                    state.pressed_keys.remove(&ra.keycode);
                    // For passthrough repeats we've been cycling press/release
                    // to the app, ending on a press.  Send a final release so
                    // the app doesn't leave the key in the held state.
                    if !ra.consumed {
                        state.send_key_release_to_app(ra.keycode);
                    }
                    tracing::debug!("repeat stopped: grace expired for key {}", ra.keycode);
                }
                state.pending_stop_release = None;
            }
        }

        // Drain any bytes queued on the timer pipe; classify by tag.
        let mut buf = [0u8; 32];
        let mut hide_status = false;
        let mut repeat_ticks: u32 = 0;
        let mut heartbeats: u32 = 0;
        loop {
            match rustix::io::read(&timer_read, &mut buf) {
                Ok(n) if n > 0 => {
                    for &b in &buf[..n] {
                        match b {
                            TAG_STATUS_HIDE => hide_status = true,
                            TAG_KEY_REPEAT => repeat_ticks += 1,
                            TAG_HEARTBEAT => heartbeats += 1,
                            _ => {}
                        }
                    }
                }
                _ => break,
            }
        }
        if heartbeats > 0 {
            tracing::info!(
                "heartbeat: ticks={heartbeats} active={} handle={} repeat_active={}",
                state.active.is_some(),
                state.handle,
                state.repeat_active.is_some()
            );
        }
        if hide_status {
            for cw in &mut state.candidate_windows {
                cw.hide_status();
            }
        }
        if repeat_ticks > 0 {
            tracing::debug!("main loop: {repeat_ticks} repeat tick(s) to process");
        }
        for _ in 0..repeat_ticks {
            let Some((keycode, keysym, was_consumed)) = state
                .repeat_active
                .as_ref()
                .map(|ra| (ra.keycode, ra.keysym, ra.consumed))
            else {
                break;
            };
            if was_consumed {
                // Re-run the IM logic — commits/preedits will fire as usual.
                // Synthetic repeat tick: there is no original wl_keyboard.key
                // serial/time, so pass 0. Engine-consumed paths never reach
                // context.key, so the placeholder is harmless for this branch.
                state.on_key(0, 0, keycode, keysym, true);
            } else {
                // Passthrough: cycle release + press so the app sees a fresh
                // keypress each tick (most apps ignore duplicate press events).
                state.send_key_release_to_app(keycode);
                state.send_key_press_to_app(keycode);
            }
        }

        // Process any buffered Wayland events and flush pending sends.
        if let Err(e) = event_queue.dispatch_pending(&mut state) {
            tracing::error!("dispatch_pending error: {e}");
            return Err(e.into());
        }
        if let Err(e) = event_queue.flush() {
            let msg = e.to_string();
            if msg.contains("Protocol error") {
                tracing::error!("wl_display flush: protocol error, exiting: {msg}");
                anyhow::bail!("Wayland protocol error during flush: {msg}");
            }
            tracing::warn!("wl_display flush error: {msg}");
        }

        // Wait for the next event (Wayland socket or timer pipe).
        let Some(guard) = event_queue.prepare_read() else {
            continue;
        };
        let wayland_fd = guard.connection_fd();
        let timer_fd = timer_read.as_fd();
        let mut fds = [
            PollFd::new(&wayland_fd, PollFlags::IN),
            PollFd::new(&timer_fd, PollFlags::IN),
        ];
        // Wait indefinitely unless there's a pending repeat-stop deadline.
        let timeout: Option<Timespec> = state.pending_stop_release.map(|d| {
            let remaining = d.saturating_duration_since(Instant::now());
            Timespec {
                tv_sec: remaining.as_secs() as _,
                tv_nsec: remaining.subsec_nanos() as _,
            }
        });
        if let Err(e) = poll(&mut fds, timeout.as_ref()) {
            tracing::warn!("poll error: {e}");
        }

        // Only read from the Wayland socket if it actually fired.
        if fds[0].revents().contains(PollFlags::IN) {
            if let Err(e) = guard.read() {
                let msg = e.to_string();
                if msg.contains("Protocol error") {
                    tracing::error!("wl_display read: protocol error, exiting: {msg}");
                    anyhow::bail!("Wayland protocol error during read: {msg}");
                }
                tracing::warn!("wl_display read error: {msg}");
            }
        }
        // Otherwise drop the guard; the timer-pipe path is handled next iteration.
    }
}
