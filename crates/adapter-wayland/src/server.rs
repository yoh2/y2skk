use std::sync::Arc;

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
}

impl WaylandState {
    fn new() -> anyhow::Result<Self> {
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

        let mut sink = ContextSink { context, serial, candidate_windows: cws, qh };
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

    fn update_status(&mut self, _indicator: &str, _timeout_ms: u32) {}
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

    let mut state = WaylandState::new()?;
    state.qh = Some(qh.clone());

    display.get_registry(&qh, ());

    // First roundtrip: enumerate all globals including wl_output.
    event_queue.roundtrip(&mut state)?;

    if state.input_method.is_none() {
        anyhow::bail!(
            "Compositor does not advertise zwp_input_method_v1. \
             Make sure you are running a Wayland compositor that supports it \
             (KDE Plasma 5/6 should work)."
        );
    }

    // Create one candidate window per output now that all outputs are known.
    state.init_candidate_windows();

    // Second roundtrip: process layer surface configure events.
    if !state.candidate_windows.is_empty() {
        event_queue.roundtrip(&mut state)?;
        tracing::info!("candidate windows ready");
    }

    tracing::info!("y2skk-wayland ready");

    loop {
        event_queue.blocking_dispatch(&mut state)?;
    }
}
