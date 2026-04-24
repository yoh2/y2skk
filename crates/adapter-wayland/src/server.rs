use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    protocol::{
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_output::WlOutput,
        wl_registry::{self, WlRegistry},
        wl_shm::WlShm,
    },
};
use wayland_protocols::wp::input_method::zv1::client::{
    zwp_input_method_context_v1::{self, ZwpInputMethodContextV1},
    zwp_input_method_v1::{self, ZwpInputMethodV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

use skk_ipc::dispatch::{ActionSink, dispatch as dispatch_actions};
use skk_ipc::proxy::reconnect::{LocalHandle, ReconnectingClient};

use crate::candidate_window::{self, CandidateWindow};
use crate::keymap;

// ── Per-activation state ──────────────────────────────────────────────────────

struct ActiveContext {
    context: ZwpInputMethodContextV1,
    keyboard: WlKeyboard,
    handle: LocalHandle,
    serial: u32,
    skk_mods: u32,
    xkb_state: Option<keymap::XkbState>,
}

// ── Top-level state ───────────────────────────────────────────────────────────

pub struct WaylandState {
    client: ReconnectingClient,
    input_method: Option<ZwpInputMethodV1>,
    active: Option<ActiveContext>,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    outputs: Vec<WlOutput>,
    pub candidate_windows: Vec<CandidateWindow>,
    qh: Option<QueueHandle<WaylandState>>,
    /// Write end of the status auto-hide pipe.  The timer thread writes to this
    /// fd after the timeout expires; the main event loop polls the read end.
    timer_write: OwnedFd,
    /// Cancel flag for the in-flight status timer thread (`Some` while a timer
    /// is pending).  Setting it to `true` makes the thread skip its write.
    status_cancel: Option<Arc<AtomicBool>>,
}

impl WaylandState {
    fn new(timer_write: OwnedFd) -> anyhow::Result<Self> {
        Ok(Self {
            client: ReconnectingClient::new()?,
            input_method: None,
            active: None,
            compositor: None,
            shm: None,
            layer_shell: None,
            outputs: Vec::new(),
            candidate_windows: Vec::new(),
            qh: None,
            timer_write,
            status_cancel: None,
        })
    }

    /// Create one CandidateWindow per known output. Called once after the first
    /// roundtrip when all globals (including outputs) have been enumerated.
    fn init_candidate_windows(&mut self) {
        let (Some(compositor), Some(shm), Some(layer_shell), Some(qh)) = (
            self.compositor.as_ref(),
            self.shm.as_ref(),
            self.layer_shell.as_ref(),
            self.qh.as_ref(),
        ) else {
            tracing::warn!("missing Wayland globals — candidate window disabled");
            return;
        };

        let Some(font) = candidate_window::load_font() else { return };
        let font = Arc::new(font);

        let outputs: Vec<Option<WlOutput>> = if self.outputs.is_empty() {
            tracing::warn!("no wl_output found — creating one candidate window (default output)");
            vec![None]
        } else {
            self.outputs.iter().map(|o| Some(o.clone())).collect()
        };

        for output_opt in outputs {
            let cw = CandidateWindow::new(
                compositor,
                layer_shell,
                shm.clone(),
                Arc::clone(&font),
                qh,
                output_opt.as_ref(),
            );
            self.candidate_windows.push(cw);
        }

        tracing::info!("created {} candidate window(s)", self.candidate_windows.len());
    }

    fn on_key(&mut self, keycode: u32, keysym: u32, is_press: bool) {
        let (handle, serial, skk_mods) = match self.active.as_ref() {
            Some(a) => (a.handle, a.serial, a.skk_mods),
            None => return,
        };

        let actions = match self.client.process_key(handle, keysym, skk_mods, is_press) {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(handle, "process_key error: {e:?}");
                return;
            }
        };

        if self.active.is_none() {
            return;
        }

        let context = &self.active.as_ref().unwrap().context;
        let qh = self.qh.as_ref();
        let cws = &mut self.candidate_windows;
        let timer_fd = self.timer_write.as_fd();
        let cancel = &mut self.status_cancel;

        let mut sink = ContextSink {
            context,
            serial,
            candidate_windows: cws,
            qh,
            timer_write_fd: timer_fd,
            status_cancel: cancel,
        };
        let result = dispatch_actions(&actions, &mut sink);

        if !result.consumed || result.force_passthrough {
            let state_val: u32 = if is_press { 1 } else { 0 };
            self.active.as_ref().unwrap().context.key(serial, 0, keycode, state_val);
        }
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
        self.context.commit_string(self.serial, text.to_string());
    }

    fn update_preedit(&mut self, text: &str, cursor: u32, _ghost_start: Option<u32>) {
        self.context.preedit_cursor(cursor as i32);
        self.context.preedit_string(self.serial, text.to_string(), String::new());
    }

    fn clear_preedit(&mut self) {
        self.context.preedit_string(self.serial, String::new(), String::new());
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
                    let _ = rustix::io::write(&fd, &[1u8]);
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
        if let wl_registry::Event::Global { name, interface, version } = event {
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
                "wl_output" => {
                    let output: WlOutput = registry.bind(name, 2.min(version), qh, ());
                    state.outputs.push(output);
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
                    state.client.destroy_handle(prev.handle);
                    prev.keyboard.release();
                    prev.context.destroy();
                }
                let handle = state.client.create_handle("wayland");
                let keyboard = id.grab_keyboard(qh, ());
                tracing::info!(handle, "input context activated");
                state.active = Some(ActiveContext {
                    context: id,
                    keyboard,
                    handle,
                    serial: 0,
                    skk_mods: 0,
                    xkb_state: None,
                });
            }
            zwp_input_method_v1::Event::Deactivate { context: _ctx } => {
                if let Some(active) = state.active.take() {
                    tracing::info!(handle = active.handle, "input context deactivated");
                    state.client.destroy_handle(active.handle);
                    active.keyboard.release();
                    // Do not call context.destroy(): KWin frees it server-side on deactivate.
                }
            }
            _ => {}
        }
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
                    active.context.preedit_string(active.serial, String::new(), String::new());
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
            wl_keyboard::Event::Key { serial: _, time: _, key, state: key_state } => {
                let is_press = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                let keysym = state.active.as_ref()
                    .and_then(|a| a.xkb_state.as_ref())
                    .map(|s| s.key_get_one_sym(key))
                    .unwrap_or(0);
                if keysym != 0 {
                    state.on_key(key, keysym, is_press);
                }
            }
            wl_keyboard::Event::Modifiers {
                serial: _,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                if let Some(active) = &mut state.active {
                    active.skk_mods = keymap::xkb_mods_to_skk(mods_depressed, mods_latched);
                    if let Some(xkb) = &mut active.xkb_state {
                        xkb.update_mask(mods_depressed, mods_latched, mods_locked, group);
                    }
                }
            }
            _ => {}
        }
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
    use rustix::event::{PollFd, PollFlags, poll};

    loop {
        // Drain any bytes queued on the timer pipe; if any fired, hide status.
        let mut buf = [0u8; 16];
        let mut timer_fired = false;
        loop {
            match rustix::io::read(&timer_read, &mut buf) {
                Ok(n) if n > 0 => timer_fired = true,
                _ => break,
            }
        }
        if timer_fired {
            for cw in &mut state.candidate_windows {
                cw.hide_status();
            }
        }

        // Process any buffered Wayland events and flush pending sends.
        event_queue.dispatch_pending(&mut state)?;
        let _ = event_queue.flush();

        // Wait for the next event (Wayland socket or timer pipe).
        let Some(guard) = event_queue.prepare_read() else { continue };
        let wayland_fd = guard.connection_fd();
        let timer_fd = timer_read.as_fd();
        let mut fds = [
            PollFd::new(&wayland_fd, PollFlags::IN),
            PollFd::new(&timer_fd, PollFlags::IN),
        ];
        // None = wait indefinitely.
        let _ = poll(&mut fds, None);

        // Only read from the Wayland socket if it actually fired.
        if fds[0].revents().contains(PollFlags::IN) {
            let _ = guard.read();
        }
        // Otherwise drop the guard; the timer-pipe path is handled next iteration.
    }
}
