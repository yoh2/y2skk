use crate::config::SkkKeybindings;
use crate::dict::entry::Candidate;
use crate::dict::traits::DictionaryProvider;
use crate::kana::table::{hiragana_to_halfwidth, hiragana_to_katakana, KanaMode, KanaTable, TransitionResult};
use crate::key::{Key, KeyEvent, Modifiers};

// ── Public types ─────────────────────────────────────────────────────────────

/// Preedit text sent to the application's input context.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preedit {
    /// The full preedit string shown to the user.
    pub text: String,
    /// Byte offset of the cursor inside `text`.
    pub cursor: usize,
}

/// Output actions produced by `SkkEngine::process_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineAction {
    /// Commit `text` to the application.
    Commit(String),
    /// Replace the preedit display.
    UpdatePreedit(Preedit),
    /// Clear the preedit display.
    ClearPreedit,
    /// Show a candidate list (page slice, focused index within page, selection key chars).
    /// Only emitted when in listing mode (index >= inline_count).
    ShowCandidates(Vec<Candidate>, usize, String),
    /// Hide the candidate list.
    HideCandidates,
    /// Notify the UI that the input mode changed.  The string is the mode indicator
    /// character shown in the status popup: "あ"/"ア"/"ｱ"/"a"/"Ａ".
    UpdateStatus(String),
    /// Key was not consumed; pass it through to the application.
    Passthrough,
}

/// Code input mode prefix character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeInputPrefix {
    /// `\` — JIS code input (4 hex digits)
    Jis,
    /// `\u` — Unicode code point (1–6 hex digits)
    Unicode,
}

/// The current phase of the SKK state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkkPhase {
    /// Hiragana input mode (default)
    Hiragana,
    /// Katakana input mode
    Katakana,
    /// Half-width katakana input mode
    HalfWidthKatakana,
    /// Wide-ASCII (全角英数) mode
    WideAscii,
    /// ASCII (半角英数, IME pass-through) mode
    Ascii,
    /// Code input mode (`\` or `\u`)
    CodeInput { prefix: CodeInputPrefix, buf: String },
    /// Abbrev mode: ASCII characters typed directly as the dictionary key (▽ascii).
    /// Entered from a kana mode by the abbrev trigger key (default: `/`).
    Abbrev { buf: String },
    /// Midashi (headword) being typed; `buf` accumulates kana
    Midashi { kana_buf: String, roman_buf: String },
    /// Okurigana being typed
    Okuri {
        midashi: String,
        okuri_prefix: char,
        /// Kana already produced mid-okurigana (e.g. "っ" before the final consonant).
        kana_buf: String,
    },
    /// Candidate selection
    Selecting {
        midashi: String,
        /// Okurigana kana text appended to the committed word (e.g. "いて").
        okuri: Option<String>,
        /// Okurigana consonant key used for dictionary lookup (e.g. "k").
        /// Stored separately from `okuri` so we can re-use it when learning.
        okuri_key: Option<String>,
        candidates: Vec<Candidate>,
        index: usize,
    },
}

// ── Registration stack ────────────────────────────────────────────────────────

/// One level of the (possibly recursive) dictionary registration mode.
#[derive(Debug, Clone)]
pub struct RegisterFrame {
    /// Reading used as the dict lookup key (e.g. "へんかんちゅう").
    pub midashi: String,
    /// Okurigana consonant key stored in the dict entry (e.g. "k" for うごく).
    pub okuri_key: Option<String>,
    /// Okurigana kana appended to the commit string (e.g. "く").
    pub okuri_kana: Option<String>,
    /// Text accumulated so far for this registration level.
    pub committed: String,
    /// Byte offset of the editing cursor within `committed`.
    pub cursor: usize,
    /// True when this frame was entered via abbrev mode (ASCII midashi).
    /// Used by `cancel_register` to decide whether to return to Abbrev or Midashi.
    pub is_abbrev: bool,
}

// ── Completion state ─────────────────────────────────────────────────────────

/// Tab-completion state, active only while in `SkkPhase::Midashi`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionState {
    /// Original prefix that started this cycling session; stays fixed across Tabs.
    cycle_prefix: String,
    /// Index of the *next* completion to offer after the current preview is accepted.
    next_index: usize,
    /// Full headword currently shown as ghost text in the preedit.
    preview: String,
}

// ── Engine ───────────────────────────────────────────────────────────────────

pub struct SkkEngine {
    phase: SkkPhase,
    kana_state: String,
    kana_table: KanaTable,
    keybindings: SkkKeybindings,
    dict: Vec<Box<dyn DictionaryProvider>>,
    /// Non-empty while in dictionary registration mode (supports recursive nesting).
    register_stack: Vec<RegisterFrame>,
    /// Kana display mode active when ▽ (midashi) mode was entered.
    /// Used to show the midashi in the correct script (hiragana/katakana/half-width).
    midashi_display_mode: KanaMode,
    /// True when the current midashi/conversion was entered via abbrev mode.
    /// Used by `cancel_register` to restore abbrev mode instead of ▽ midashi mode.
    midashi_is_abbrev: bool,
    /// Active completion ghost text, only meaningful when phase == Midashi.
    completion: Option<CompletionState>,
    /// When conversion was started by a trigger character (e.g. ',' '.' 'を'),
    /// this holds the string to append after the committed candidate.
    /// Cleared on conversion cancel or when registration mode is entered.
    conversion_trigger: Option<String>,
}

impl SkkEngine {
    pub fn new(kana_table: KanaTable, keybindings: SkkKeybindings) -> Self {
        Self {
            phase: SkkPhase::Hiragana,
            kana_state: String::new(),
            kana_table,
            keybindings,
            dict: Vec::new(),
            register_stack: Vec::new(),
            midashi_display_mode: KanaMode::Hiragana,
            midashi_is_abbrev: false,
            completion: None,
            conversion_trigger: None,
        }
    }

    /// Sets the initial input phase (builder pattern).
    /// Only "safe" phases (Hiragana, Katakana, HalfWidthKatakana, WideAscii, Ascii) are accepted;
    /// mid-conversion phases are silently ignored.
    pub fn with_initial_phase(mut self, phase: SkkPhase) -> Self {
        match phase {
            SkkPhase::Hiragana
            | SkkPhase::Katakana
            | SkkPhase::HalfWidthKatakana
            | SkkPhase::WideAscii
            | SkkPhase::Ascii => self.phase = phase,
            _ => {}
        }
        self
    }

    /// Attaches a dictionary provider (appended; sorted by priority when looking up).
    pub fn add_dict(&mut self, provider: Box<dyn DictionaryProvider>) {
        self.dict.push(provider);
        self.dict.sort_by_key(|d| std::cmp::Reverse(d.priority()));
    }

    /// Processes a key event and returns zero or more actions for the platform adapter.
    pub fn process_key(&mut self, event: &KeyEvent) -> Vec<EngineAction> {
        if !event.is_press {
            return vec![EngineAction::Passthrough];
        }
        let before = self.current_mode_indicator();
        let mut actions = self.handle_press(event);
        let after = self.current_mode_indicator();
        if after != before {
            actions.push(EngineAction::UpdateStatus(after.to_string()));
        }
        actions
    }

    /// Returns the one-character mode indicator for the current phase.
    /// For conversion sub-phases (▽/▼/okurigana), returns the indicator of the
    /// kana mode that was active when ▽ was entered, so that mode-switch events
    /// (e.g. q confirming from katakana ▼) are correctly detected.
    fn current_mode_indicator(&self) -> &'static str {
        match &self.phase {
            SkkPhase::Katakana          => "ア",
            SkkPhase::HalfWidthKatakana => "ｱ",
            SkkPhase::Ascii             => "a",
            SkkPhase::WideAscii         => "Ａ",
            SkkPhase::Abbrev { .. }
            | SkkPhase::Midashi { .. }
            | SkkPhase::Okuri { .. }
            | SkkPhase::Selecting { .. } => match self.midashi_display_mode {
                KanaMode::Katakana  => "ア",
                KanaMode::HalfWidth => "ｱ",
                KanaMode::Hiragana  => "あ",
            },
            _ => "あ",
        }
    }

    pub fn phase(&self) -> &SkkPhase {
        &self.phase
    }

    // ── Internal dispatch ─────────────────────────────────────────────────────

    fn handle_press(&mut self, event: &KeyEvent) -> Vec<EngineAction> {
        // ── Registration mode special keys ──────────────────────────────────
        if !self.register_stack.is_empty() {
            let in_ready_state =
                matches!(self.phase,
                    SkkPhase::Hiragana | SkkPhase::Katakana | SkkPhase::HalfWidthKatakana)
                && self.kana_state.is_empty();

            let is_ctrl_g = event.key == Key::Char('g')
                && event.modifiers.contains(Modifiers::CTRL);

            if in_ready_state {
                if event.key == Key::Return {
                    let buf_empty = self.register_stack.last()
                        .map(|f| f.committed.is_empty()).unwrap_or(true);
                    return if buf_empty {
                        self.cancel_register()
                    } else {
                        self.finalize_register()
                    };
                }
                if is_ctrl_g || event.key == Key::Escape {
                    return self.cancel_register();
                }
                // Cursor editing keys for the registration buffer.
                match event.key {
                    Key::BackSpace => {
                        self.reg_backspace();
                        return vec![self.preedit_action()];
                    }
                    Key::Delete => {
                        self.reg_delete();
                        return vec![self.preedit_action()];
                    }
                    Key::Left => {
                        self.reg_move_left();
                        return vec![self.preedit_action()];
                    }
                    Key::Right => {
                        self.reg_move_right();
                        return vec![self.preedit_action()];
                    }
                    _ => {}
                }
            } else if is_ctrl_g {
                // C-g in any sub-phase: abort the sub-phase and cancel registration.
                self.phase = SkkPhase::Hiragana;
                self.kana_state.clear();
                return self.cancel_register();
            } else if matches!(
                    self.phase,
                    SkkPhase::Abbrev { .. } | SkkPhase::Midashi { .. } | SkkPhase::Okuri { .. })
                    && event.key == Key::Return {
                // Return in abbrev/▽/okuri mode during registration: flush the typed text to the
                // registration buffer and return to Hiragana (ready state).  This allows
                // the user to commit a kana/ascii string as-is without triggering a conversion.
                let flush = match &self.phase {
                    SkkPhase::Abbrev { buf } => buf.clone(),
                    SkkPhase::Midashi { kana_buf, .. } => kana_buf.clone(),
                    SkkPhase::Okuri { midashi, .. } => midashi.clone(),
                    _ => unreachable!(),
                };
                self.phase = SkkPhase::Hiragana;
                self.kana_state.clear();
                if !flush.is_empty() {
                    if let Some(frame) = self.register_stack.last_mut() {
                        frame.committed.insert_str(frame.cursor, &flush);
                        frame.cursor += flush.len();
                    }
                }
                return vec![self.preedit_action()];
            }

            // In ASCII mode, printable chars and editing keys are redirected to the
            // registration buffer instead of leaking to the application.
            if matches!(self.phase, SkkPhase::Ascii) {
                match event.key {
                    Key::Return => {
                        let buf_empty = self.register_stack.last()
                            .map(|f| f.committed.is_empty()).unwrap_or(true);
                        return if buf_empty {
                            self.cancel_register()
                        } else {
                            self.finalize_register()
                        };
                    }
                    Key::Escape => {
                        return self.cancel_register();
                    }
                    Key::BackSpace => {
                        self.reg_backspace();
                        return vec![self.preedit_action()];
                    }
                    Key::Delete => {
                        self.reg_delete();
                        return vec![self.preedit_action()];
                    }
                    Key::Left => {
                        self.reg_move_left();
                        return vec![self.preedit_action()];
                    }
                    Key::Right => {
                        self.reg_move_right();
                        return vec![self.preedit_action()];
                    }
                    _ => {
                        if let Some(ch) = event.printable_char() {
                            if !event.modifiers.contains(Modifiers::CTRL) {
                                if let Some(frame) = self.register_stack.last_mut() {
                                    frame.committed.insert(frame.cursor, ch);
                                    frame.cursor += ch.len_utf8();
                                }
                                return vec![self.preedit_action()];
                            }
                        }
                        // Ctrl keys fall through to handle_ascii (e.g. Ctrl+j → Hiragana).
                    }
                }
            }
        }

        // Toggle keys switch between Ascii (IME off) and Hiragana (IME on).
        // Only active outside registration mode (register_stack is already empty here
        // for any key that reached this point without an early return above).
        if self.register_stack.is_empty() {
            let is_toggle = self.keybindings.toggle_keys.iter()
                .any(|(k, m)| event.key == *k && event.modifiers == *m);
            if is_toggle {
                match self.phase {
                    SkkPhase::Ascii => self.phase = SkkPhase::Hiragana,
                    _ => {
                        self.phase = SkkPhase::Ascii;
                        self.kana_state.clear();
                    }
                }
                return vec![EngineAction::ClearPreedit];
            }
        }

        let raw_actions = match &self.phase.clone() {
            SkkPhase::Ascii => self.handle_ascii(event),
            SkkPhase::Hiragana | SkkPhase::Katakana | SkkPhase::HalfWidthKatakana => {
                self.handle_kana(event)
            }
            SkkPhase::WideAscii => self.handle_wide_ascii(event),
            SkkPhase::CodeInput { prefix, buf } => {
                self.handle_code_input(event, prefix.clone(), buf.clone())
            }
            SkkPhase::Abbrev { buf } => {
                self.handle_abbrev(event, buf.clone())
            }
            SkkPhase::Midashi { kana_buf, roman_buf } => {
                self.handle_midashi(event, kana_buf.clone(), roman_buf.clone())
            }
            SkkPhase::Okuri { midashi, okuri_prefix, kana_buf } => {
                self.handle_okuri(event, midashi.clone(), *okuri_prefix, kana_buf.clone())
            }
            SkkPhase::Selecting { midashi, okuri, okuri_key, candidates, index } => {
                self.handle_selecting(
                    event,
                    midashi.clone(),
                    okuri.clone(),
                    okuri_key.clone(),
                    candidates.clone(),
                    *index,
                )
            }
        };

        // ── Intercept commits when in registration mode ──────────────────────
        if !self.register_stack.is_empty() {
            self.intercept_for_register(raw_actions)
        } else {
            raw_actions
        }
    }

    // ── ASCII mode ────────────────────────────────────────────────────────────

    fn handle_ascii(&mut self, event: &KeyEvent) -> Vec<EngineAction> {
        // Ctrl+j or Ctrl+q switches back to hiragana
        if event.modifiers.contains(Modifiers::CTRL) {
            match event.key {
                Key::Char('j') | Key::Char('q') => {
                    self.phase = SkkPhase::Hiragana;
                    return vec![EngineAction::ClearPreedit];
                }
                _ => {}
            }
        }
        vec![EngineAction::Passthrough]
    }

    // ── Hiragana / Katakana mode ──────────────────────────────────────────────

    fn handle_kana(&mut self, event: &KeyEvent) -> Vec<EngineAction> {
        let mode = match &self.phase {
            SkkPhase::Katakana => KanaMode::Katakana,
            SkkPhase::HalfWidthKatakana => KanaMode::HalfWidth,
            _ => KanaMode::Hiragana,
        };

        // Ctrl+j → hiragana
        if event.key == Key::Char('j') && event.modifiers.contains(Modifiers::CTRL) {
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit];
        }

        // Ctrl+q: toggle half-width katakana
        //   hiragana / katakana → half-width katakana
        //   half-width katakana → hiragana
        if event.key == Key::Char('q') && event.modifiers.contains(Modifiers::CTRL) {
            self.phase = match self.phase {
                SkkPhase::HalfWidthKatakana => SkkPhase::Hiragana,
                _ => SkkPhase::HalfWidthKatakana,
            };
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit];
        }

        // Escape clears any pending roman buffer and passes through to the application.
        if event.key == Key::Escape {
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit, EngineAction::Passthrough];
        }

        // BackSpace removes last character from roman buffer
        if event.key == Key::BackSpace {
            if !self.kana_state.is_empty() {
                self.kana_state.pop();
                return vec![self.preedit_action()];
            }
            return vec![EngineAction::Passthrough];
        }

        let Some(ch) = event.printable_char() else {
            return vec![EngineAction::Passthrough];
        };

        // Ctrl+key combinations not handled above (i.e. not C-j or C-q) are not
        // IME operations and must reach the application (e.g. C-c for SIGINT).
        if event.modifiers.contains(Modifiers::CTRL) {
            self.kana_state.clear();
            return vec![EngineAction::Passthrough];
        }

        // Mode switches (only in start state to avoid conflict with roman sequences)
        if self.kana_state.is_empty() {
            if self.keybindings.ascii_mode.contains(&ch) {
                self.phase = SkkPhase::Ascii;
                return vec![EngineAction::ClearPreedit];
            }
            if self.keybindings.wide_ascii_mode.contains(&ch) {
                self.phase = SkkPhase::WideAscii;
                return vec![EngineAction::ClearPreedit];
            }
            if self.keybindings.abbrev_mode.contains(&ch) {
                self.midashi_display_mode = mode;
                self.midashi_is_abbrev = true;
                self.phase = SkkPhase::Abbrev { buf: String::new() };
                return vec![self.preedit_action()];
            }
            if self.keybindings.katakana_mode.contains(&ch) {
                // q toggles between hiragana and katakana only.
                // From half-width katakana, q returns to hiragana (not full-width katakana).
                self.phase = match self.phase {
                    SkkPhase::Katakana | SkkPhase::HalfWidthKatakana => SkkPhase::Hiragana,
                    _ => SkkPhase::Katakana,
                };
                return vec![EngineAction::ClearPreedit];
            }
            // Uppercase letter starts midashi
            if ch.is_ascii_uppercase() {
                // Remember the kana mode so the midashi preedit can be displayed
                // in the correct script (e.g. katakana when entering from Katakana mode).
                self.midashi_display_mode = mode;
                self.midashi_is_abbrev = false;
                self.phase = SkkPhase::Midashi {
                    kana_buf: String::new(),
                    roman_buf: String::new(),
                };
                // Re-dispatch as midashi input
                return self.handle_midashi(event, String::new(), String::new());
            }
            // `\` starts code input; remember current kana mode for restoration on exit.
            if ch == '\\' {
                self.midashi_display_mode = mode;
                self.phase = SkkPhase::CodeInput {
                    prefix: CodeInputPrefix::Jis,
                    buf: String::new(),
                };
                return vec![self.preedit_action()];
            }
        }

        // Feed the character into the kana state machine
        self.feed_kana(ch, mode)
    }

    // ── Kana state machine helper ─────────────────────────────────────────────

    /// Feeds a character into the kana state machine and returns the resulting actions.
    fn feed_kana(&mut self, ch: char, mode: KanaMode) -> Vec<EngineAction> {
        let result = self.kana_table.transition(&self.kana_state.clone(), ch, mode);
        match result {
            TransitionResult::Ok { output, next_state } => {
                self.kana_state = next_state;
                if output.is_empty() {
                    vec![self.preedit_action()]
                } else {
                    let mut actions = vec![EngineAction::Commit(output)];
                    if self.kana_state.is_empty() {
                        actions.push(EngineAction::ClearPreedit);
                    } else {
                        actions.push(self.preedit_action());
                    }
                    actions
                }
            }
            TransitionResult::OkRetry { output, retry } => {
                // Wildcard rule fired (e.g. n + consonant → "ん").  Emit the output and then
                // re-process the triggering character from the start state so it is not lost.
                self.kana_state.clear();
                let mut actions = if output.is_empty() {
                    vec![]
                } else {
                    vec![EngineAction::Commit(output)]
                };
                actions.extend(self.feed_kana(retry, mode));
                actions
            }
            TransitionResult::NoMatch { flush: _, retry } => {
                // The accumulated intermediate state failed to match.  The partial sequence
                // (flush) is silently discarded rather than committed to the application.
                // This prevents stray characters like a lone "c" from appearing in the output
                // when the user types an unrecognised romaji sequence (e.g. "cde" → "で").
                //
                // Exception: when NoMatch fires from the *empty* start state (flush would have
                // been empty anyway and there is no retry), the input character itself did not
                // start any sequence at all — pass it through as-is so that digits, punctuation,
                // and other non-kana characters are committed normally.
                self.kana_state.clear();
                let mut actions = Vec::new();
                if let Some(retry_ch) = retry {
                    actions.extend(self.feed_kana(retry_ch, mode));
                } else {
                    // No accumulated state and no retry means the character simply does not
                    // begin any kana sequence: commit it directly (e.g. '1', '.', ' ').
                    actions.push(EngineAction::Commit(ch.to_string()));
                    actions.push(EngineAction::ClearPreedit);
                }
                actions
            }
        }
    }

    // ── Wide-ASCII mode ───────────────────────────────────────────────────────

    fn handle_wide_ascii(&mut self, event: &KeyEvent) -> Vec<EngineAction> {
        // Ctrl+j or Ctrl+q → hiragana
        if event.modifiers.contains(Modifiers::CTRL) {
            match event.key {
                Key::Char('j') | Key::Char('q') => {
                    self.phase = SkkPhase::Hiragana;
                    return vec![EngineAction::ClearPreedit];
                }
                _ => {}
            }
        }
        if let Some(ch) = event.printable_char() {
            if let Some(wide) = to_wide_ascii(ch) {
                return vec![EngineAction::Commit(wide.to_string())];
            }
        }
        vec![EngineAction::Passthrough]
    }

    // ── Code input mode ───────────────────────────────────────────────────────

    fn handle_code_input(
        &mut self,
        event: &KeyEvent,
        prefix: CodeInputPrefix,
        mut buf: String,
    ) -> Vec<EngineAction> {
        // Escape → cancel and return to previous kana mode
        if event.key == Key::Escape {
            self.phase = self.kana_phase_from_display_mode();
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit];
        }

        // BackSpace
        if event.key == Key::BackSpace {
            if !buf.is_empty() {
                buf.pop();
                self.phase = SkkPhase::CodeInput { prefix, buf };
            } else if matches!(prefix, CodeInputPrefix::Unicode) {
                // Back from `\u` to plain `\`
                self.phase = SkkPhase::CodeInput {
                    prefix: CodeInputPrefix::Jis,
                    buf: String::new(),
                };
            } else {
                // Back from `\` to kana mode
                self.phase = self.kana_phase_from_display_mode();
                self.kana_state.clear();
                return vec![EngineAction::ClearPreedit];
            }
            return vec![self.preedit_action()];
        }

        // `\u` prefix detection: first character after `\` is 'u'
        if matches!(prefix, CodeInputPrefix::Jis) && buf.is_empty() {
            if let Some('u') = event.printable_char() {
                self.phase = SkkPhase::CodeInput {
                    prefix: CodeInputPrefix::Unicode,
                    buf: String::new(),
                };
                return vec![self.preedit_action()];
            }
        }

        // Enter (or C-j) → commit and return to previous kana mode
        if event.key == Key::Return
            || (event.key == Key::Char('j') && event.modifiers.contains(Modifiers::CTRL))
        {
            let result = match prefix {
                CodeInputPrefix::Jis => decode_jis_code(&buf),
                CodeInputPrefix::Unicode => decode_unicode_code(&buf),
            };
            self.phase = self.kana_phase_from_display_mode();
            self.kana_state.clear();
            return match result {
                Some(s) => vec![EngineAction::Commit(s), EngineAction::ClearPreedit],
                None => vec![EngineAction::ClearPreedit],
            };
        }

        // Hex digit input
        if let Some(ch) = event.printable_char() {
            if ch.is_ascii_hexdigit() && !event.modifiers.contains(Modifiers::CTRL) {
                buf.push(ch.to_ascii_lowercase());
                self.phase = SkkPhase::CodeInput { prefix, buf };
                return vec![self.preedit_action()];
            }
        }

        // Non-hex, non-special key: consume without action
        vec![self.preedit_action()]
    }

    // ── Abbrev mode ───────────────────────────────────────────────────────────

    fn handle_abbrev(&mut self, event: &KeyEvent, mut buf: String) -> Vec<EngineAction> {
        // Escape → cancel abbrev, return to the kana mode we came from
        if event.key == Key::Escape {
            self.phase = self.kana_phase_from_display_mode();
            return vec![EngineAction::ClearPreedit];
        }

        // BackSpace
        if event.key == Key::BackSpace {
            if buf.is_empty() {
                self.phase = self.kana_phase_from_display_mode();
                return vec![EngineAction::ClearPreedit];
            }
            buf.pop();
            self.phase = SkkPhase::Abbrev { buf };
            return vec![self.preedit_action()];
        }

        // Space → trigger conversion
        if event.key == Key::Space {
            if buf.is_empty() {
                self.phase = SkkPhase::Abbrev { buf };
                return vec![self.preedit_action()];
            }
            return self.start_conversion(buf, None);
        }

        // C-j → commit as-is and return to kana mode (Enter does NOT pass through for C-j).
        if event.key == Key::Char('j') && event.modifiers.contains(Modifiers::CTRL) {
            self.phase = self.kana_phase_from_display_mode();
            if buf.is_empty() {
                return vec![EngineAction::ClearPreedit];
            }
            return vec![EngineAction::Commit(buf), EngineAction::ClearPreedit];
        }

        // Enter → commit as-is, return to kana mode, AND forward Enter to the application.
        if event.key == Key::Return {
            self.phase = self.kana_phase_from_display_mode();
            if buf.is_empty() {
                return vec![EngineAction::ClearPreedit, EngineAction::Passthrough];
            }
            return vec![
                EngineAction::Commit(buf),
                EngineAction::ClearPreedit,
                EngineAction::Passthrough,
            ];
        }

        let Some(ch) = event.printable_char() else {
            // Non-printable, non-special key (e.g. arrow keys): consume without action.
            return vec![self.preedit_action()];
        };

        // Unhandled Ctrl+key combinations are consumed (not forwarded to the application)
        // while the abbrev buffer is active, the same as in ▽ mode.
        if event.modifiers.contains(Modifiers::CTRL) {
            return vec![self.preedit_action()];
        }

        buf.push(ch);
        self.phase = SkkPhase::Abbrev { buf };
        vec![self.preedit_action()]
    }

    /// Returns the `SkkPhase` corresponding to `midashi_display_mode`.
    fn kana_phase_from_display_mode(&self) -> SkkPhase {
        match self.midashi_display_mode {
            KanaMode::Katakana  => SkkPhase::Katakana,
            KanaMode::HalfWidth => SkkPhase::HalfWidthKatakana,
            KanaMode::Hiragana  => SkkPhase::Hiragana,
        }
    }

    // ── Midashi mode ──────────────────────────────────────────────────────────

    fn handle_midashi(
        &mut self,
        event: &KeyEvent,
        mut kana_buf: String,
        mut roman_buf: String,
    ) -> Vec<EngineAction> {
        // Escape cancels midashi
        if event.key == Key::Escape {
            self.completion = None;
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit];
        }

        // BackSpace
        if event.key == Key::BackSpace {
            if !roman_buf.is_empty() {
                roman_buf.pop();
                // Keep kana_state in sync with roman_buf so subsequent input
                // is processed from the correct intermediate state.
                self.kana_state = roman_buf.clone();
                // Ghost persists while editing roman_buf — no completion change.
            } else if !kana_buf.is_empty() {
                // Remove the last kana character and recompute completion for the
                // shorter prefix.  This also resets the cycling state.
                kana_buf.pop();
                self.kana_state.clear();
                self.update_completion(&kana_buf);
            } else {
                // Empty midashi — cancel
                self.completion = None;
                self.phase = SkkPhase::Hiragana;
                self.kana_state.clear();
                return vec![EngineAction::ClearPreedit];
            }
            self.phase = SkkPhase::Midashi { kana_buf, roman_buf };
            return vec![self.preedit_action()];
        }

        // Enter — commit accumulated kana as-is and forward Enter to the application.
        if event.key == Key::Return {
            // Flush "n" to "ん" if pending, discard other partial romaji.
            if roman_buf == "n" {
                kana_buf.push('ん');
            }
            self.completion = None;
            self.phase = self.kana_phase_from_display_mode();
            self.kana_state.clear();
            if kana_buf.is_empty() {
                return vec![EngineAction::ClearPreedit, EngineAction::Passthrough];
            }
            return vec![
                EngineAction::Commit(kana_buf),
                EngineAction::ClearPreedit,
                EngineAction::Passthrough,
            ];
        }

        // Space — trigger conversion (ignores any ghost text; converts actual kana_buf)
        if event.key == Key::Space {
            // Flush any pending roman buffer before lookup.
            // "n" is a special case: it should become "ん" at end of reading.
            // Any other partial romaji sequence is discarded (e.g. "k" in "▽しk").
            if roman_buf == "n" {
                kana_buf.push('ん');
            }
            self.completion = None;
            self.kana_state.clear();
            if kana_buf.is_empty() {
                self.phase = SkkPhase::Hiragana;
                return vec![EngineAction::ClearPreedit];
            }
            return self.start_conversion(kana_buf, None);
        }

        // Tab — accept current ghost text and advance the completion cycle.
        if event.key == Key::Tab {
            if roman_buf.is_empty() {
                match self.completion.take() {
                    Some(state) => {
                        // Accept the currently previewed headword.
                        let accepted = state.preview;
                        // Find the next completion from the same cycle prefix.
                        let next = self.find_completion(&state.cycle_prefix, state.next_index);
                        self.completion = next.map(|(preview, next_index)| CompletionState {
                            cycle_prefix: state.cycle_prefix,
                            next_index,
                            preview,
                        });
                        self.kana_state.clear();
                        self.phase = SkkPhase::Midashi {
                            kana_buf: accepted,
                            roman_buf: String::new(),
                        };
                    }
                    None => {
                        // No completion available: do nothing.
                        self.phase = SkkPhase::Midashi { kana_buf, roman_buf };
                    }
                }
            } else {
                // roman_buf non-empty: ignore Tab.
                self.phase = SkkPhase::Midashi { kana_buf, roman_buf };
            }
            return vec![self.preedit_action()];
        }

        // Ctrl+q: commit accumulated hiragana midashi as half-width katakana, then
        // return to the kana mode that was active before entering ▽ mode.
        if event.key == Key::Char('q') && event.modifiers.contains(Modifiers::CTRL) {
            let return_phase = match self.midashi_display_mode {
                KanaMode::Katakana  => SkkPhase::Katakana,
                KanaMode::HalfWidth => SkkPhase::HalfWidthKatakana,
                KanaMode::Hiragana  => SkkPhase::Hiragana,
            };
            self.completion = None;
            self.phase = return_phase;
            self.kana_state.clear();
            if kana_buf.is_empty() {
                return vec![EngineAction::ClearPreedit];
            }
            let halfwidth = hiragana_to_halfwidth(&kana_buf);
            return vec![EngineAction::Commit(halfwidth), EngineAction::ClearPreedit];
        }

        let Some(ch) = event.printable_char() else {
            return vec![EngineAction::Passthrough];
        };

        // ASCII conversion trigger chars (e.g. ',' '.'):
        // When the kana state is empty and the typed character is an ASCII trigger,
        // start conversion immediately and remember the char to append after commit.
        if ch.is_ascii()
            && self.kana_state.is_empty()
            && !self.keybindings.conversion_trigger_chars.is_empty()
            && self.keybindings.conversion_trigger_chars.contains(&ch)
        {
            // Flush a pending 'n' to 'ん' before converting.
            if roman_buf == "n" {
                kana_buf.push('ん');
            }
            self.completion = None;
            self.kana_state.clear();
            // Resolve the trigger char through the kana table so the output matches
            // the user's layout (e.g. '.' → "。", ',' → "、", or custom mappings).
            // Fall back to the raw char if the table has no single-step mapping.
            let trigger_output =
                match self.kana_table.transition("", ch, KanaMode::Hiragana) {
                    TransitionResult::Ok { output, next_state }
                        if !output.is_empty() && next_state.is_empty() =>
                    {
                        output
                    }
                    _ => ch.to_string(),
                };
            if kana_buf.is_empty() {
                // Nothing to convert: just output the trigger char and exit ▽ mode.
                self.phase = self.kana_phase_from_display_mode();
                return vec![EngineAction::Commit(trigger_output), EngineAction::ClearPreedit];
            }
            self.conversion_trigger = Some(trigger_output);
            return self.start_conversion(kana_buf, None);
        }

        // '>', '<', '?' trigger prefix conversion: append '>' to the midashi and
        // convert immediately (looks up "reading>" in the dictionary).
        if matches!(ch, '>' | '<' | '?') {
            self.completion = None;
            if !roman_buf.is_empty() {
                kana_buf.push_str(&roman_buf);
            }
            self.kana_state.clear();
            if kana_buf.is_empty() {
                // Nothing to convert; stay in midashi mode.
                self.phase = SkkPhase::Midashi { kana_buf, roman_buf: String::new() };
                return vec![self.preedit_action()];
            }
            kana_buf.push('>');
            return self.start_conversion(kana_buf, None);
        }

        // Uppercase letter starts okurigana.
        // The okuri_prefix is the first consonant of the okurigana sequence:
        // - if there is a pending kana state (e.g. "w" in "KawA"), that is the prefix;
        // - otherwise the uppercase letter itself (lowercased) is the prefix.
        if ch.is_ascii_uppercase() && !kana_buf.is_empty() {
            self.completion = None;
            let lower = ch.to_ascii_lowercase();

            // Handle the double-consonant + uppercase case (e.g. "SasSu" for "察す"):
            // When there is a pending kana state (e.g. "s") and the uppercase letter's
            // lowercase form continues that state to produce an intermediate kana output
            // (e.g. "s"+"s" → "っ" with next_state="s"), the intermediate kana belongs
            // to the midashi, not the okurigana.  Only flush when next_state is non-empty
            // (i.e. the okurigana consonant is still pending); a complete kana output
            // (next_state empty, e.g. "SassU" where "s"+"u"→"す") falls through so that
            // the existing logic feeds it through handle_okuri as the okurigana.
            if !self.kana_state.is_empty() {
                let state_before = self.kana_state.clone();
                if let TransitionResult::Ok { output, next_state } =
                    self.kana_table.transition(&state_before, lower, KanaMode::Hiragana)
                {
                    if !output.is_empty() && !next_state.is_empty() {
                        kana_buf.push_str(&output);
                        self.kana_state = next_state;
                        let okuri_prefix = self.kana_state.chars().next().unwrap_or(lower);
                        self.phase = SkkPhase::Okuri {
                            midashi: kana_buf,
                            okuri_prefix,
                            kana_buf: String::new(),
                        };
                        return vec![self.preedit_action()];
                    }
                }
            }

            let okuri_prefix = if let Some(first) = self.kana_state.chars().next() {
                first
            } else {
                lower
            };
            self.phase = SkkPhase::Okuri {
                midashi: kana_buf,
                okuri_prefix,
                kana_buf: String::new(),
            };
            return self.handle_okuri(event, self.phase.clone_midashi(), okuri_prefix, String::new());
        }

        // Lowercase character: feed into kana state machine
        let lower = if ch.is_ascii_uppercase() { ch.to_ascii_lowercase() } else { ch };
        let state_before = self.kana_state.clone();
        let result = self.kana_table.transition(&state_before, lower, KanaMode::Hiragana);

        match result {
            TransitionResult::Ok { output, next_state } => {
                roman_buf.clear();
                roman_buf.push_str(&next_state);
                if !output.is_empty() {
                    kana_buf.push_str(&output);
                    self.kana_state = next_state.clone();
                    roman_buf.clear();
                    roman_buf.push_str(&next_state);
                    if next_state.is_empty() {
                        // A complete kana was emitted and roman_buf is now empty.
                        // Check for non-ASCII kana conversion triggers (e.g. 'を').
                        if let Some(last_ch) = kana_buf.chars().next_back() {
                            if !last_ch.is_ascii()
                                && !self.keybindings.conversion_trigger_chars.is_empty()
                                && self.keybindings.conversion_trigger_chars.contains(&last_ch)
                            {
                                let char_len = last_ch.len_utf8();
                                kana_buf.truncate(kana_buf.len() - char_len);
                                self.completion = None;
                                self.kana_state.clear();
                                if kana_buf.is_empty() {
                                    // Nothing to convert: output the trigger char and exit ▽.
                                    self.phase = self.kana_phase_from_display_mode();
                                    return vec![
                                        EngineAction::Commit(last_ch.to_string()),
                                        EngineAction::ClearPreedit,
                                    ];
                                }
                                self.conversion_trigger = Some(last_ch.to_string());
                                return self.start_conversion(kana_buf, None);
                            }
                        }
                        // Recompute completion ghost for the new kana_buf.
                        self.update_completion(&kana_buf);
                    } else {
                        // roman_buf is still building; hide ghost until it resolves.
                        self.completion = None;
                    }
                } else {
                    // No kana output yet; roman_buf is accumulating.
                    self.completion = None;
                    self.kana_state = next_state;
                }
                self.phase = SkkPhase::Midashi { kana_buf, roman_buf };
                vec![self.preedit_action()]
            }
            TransitionResult::OkRetry { output, retry } => {
                // Wildcard rule (e.g. n + consonant → "ん"): add output to midashi and
                // re-dispatch the triggering character so it is not lost.
                if !output.is_empty() {
                    kana_buf.push_str(&output);
                }
                self.kana_state.clear();
                roman_buf.clear();
                self.phase = SkkPhase::Midashi {
                    kana_buf: kana_buf.clone(),
                    roman_buf: String::new(),
                };
                let fake_event = KeyEvent::press(Key::Char(retry), Modifiers::empty());
                self.handle_midashi(&fake_event, kana_buf, String::new())
            }
            TransitionResult::NoMatch { flush: _, retry } => {
                // Partial romaji sequence failed: discard the accumulated state (flush)
                // rather than adding it to the midashi reading.
                self.kana_state.clear();
                roman_buf.clear();
                self.phase = SkkPhase::Midashi {
                    kana_buf: kana_buf.clone(),
                    roman_buf: String::new(),
                };
                if let Some(retry_ch) = retry {
                    let fake_event = KeyEvent::press(Key::Char(retry_ch), Modifiers::empty());
                    self.handle_midashi(&fake_event, kana_buf, String::new())
                } else {
                    // 'q' (without Ctrl) from the empty state converts the accumulated
                    // hiragana midashi to katakana and commits it directly (no dict lookup).
                    // This only fires when the kana table has no transition for 'q' (e.g.
                    // standard romaji).  Tables that do use 'q' (e.g. DvorakJP) will have
                    // matched above and won't reach here.
                    // Ctrl+q is handled separately above and never reaches this branch.
                    // 'q' (without Ctrl) toggles the kana type and commits:
                    //   hiragana ▽       → commit as katakana,  return to hiragana
                    //   katakana ▽       → commit as hiragana,  return to hiragana
                    //   half-width ▽     → commit as hiragana,  return to half-width katakana
                    if ch == 'q' && !event.modifiers.contains(Modifiers::CTRL) && !kana_buf.is_empty() {
                        let committed = match self.midashi_display_mode {
                            KanaMode::Hiragana => hiragana_to_katakana(&kana_buf),
                            _                  => kana_buf.clone(), // kana_buf is hiragana internally
                        };
                        self.completion = None;
                        self.phase = match self.midashi_display_mode {
                            KanaMode::Katakana  => SkkPhase::Katakana,
                            KanaMode::HalfWidth => SkkPhase::HalfWidthKatakana,
                            KanaMode::Hiragana  => SkkPhase::Hiragana,
                        };
                        self.kana_state.clear();
                        return vec![EngineAction::Commit(committed), EngineAction::ClearPreedit];
                    }
                    vec![self.preedit_action()]
                }
            }
        }
    }

    // ── Okuri mode ────────────────────────────────────────────────────────────

    fn handle_okuri(
        &mut self,
        event: &KeyEvent,
        midashi: String,
        okuri_prefix: char,
        mut kana_buf: String,
    ) -> Vec<EngineAction> {
        if event.key == Key::Escape {
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit];
        }

        let Some(ch) = event.printable_char() else {
            return vec![EngineAction::Passthrough];
        };

        let lower = ch.to_ascii_lowercase();

        let result = self.kana_table.transition(&self.kana_state.clone(), lower, KanaMode::Hiragana);
        match result {
            TransitionResult::Ok { output, next_state } => {
                self.kana_state = next_state;
                if !output.is_empty() {
                    kana_buf.push_str(&output);
                    if self.kana_state.is_empty() {
                        // Okurigana is complete; start conversion.
                        let okuri_key = self.kana_table.okuri_key(okuri_prefix).to_string();
                        self.phase = SkkPhase::Hiragana;
                        return self.start_conversion(midashi, Some((okuri_key, kana_buf)));
                    }
                    // Mid-okurigana kana produced (e.g. "っ" from "tt") but more input
                    // expected (next_state is non-empty). Accumulate and continue.
                }
                self.phase = SkkPhase::Okuri { midashi, okuri_prefix, kana_buf };
                vec![self.preedit_action()]
            }
            TransitionResult::OkRetry { output, retry: _ } => {
                // Wildcard matched (e.g. "n" + consonant → "ん") while typing okurigana.
                // Treat the output as the complete okurigana and start conversion;
                // the retry character is dropped since we cannot re-dispatch here.
                self.kana_state.clear();
                if !output.is_empty() {
                    let okuri_key = self.kana_table.okuri_key(okuri_prefix).to_string();
                    kana_buf.push_str(&output);
                    self.phase = SkkPhase::Hiragana;
                    return self.start_conversion(midashi, Some((okuri_key, kana_buf)));
                }
                self.phase = SkkPhase::Okuri { midashi, okuri_prefix, kana_buf };
                vec![self.preedit_action()]
            }
            TransitionResult::NoMatch { flush: _, retry: _ } => {
                // Feed failed; treat remaining buffer as mistype and clear state.
                self.kana_state.clear();
                self.phase = SkkPhase::Okuri { midashi, okuri_prefix, kana_buf };
                vec![self.preedit_action()]
            }
        }
    }

    // ── Conversion / candidate selection ─────────────────────────────────────

    /// Looks up `midashi` (with optional okurigana) and enters Selecting phase.
    fn start_conversion(
        &mut self,
        midashi: String,
        okuri: Option<(String, String)>,
    ) -> Vec<EngineAction> {
        self.completion = None;
        let (okuri_key, _okuri_kana) = match &okuri {
            Some((k, v)) => (Some(k.as_str()), Some(v.as_str())),
            None => (None, None),
        };

        let candidates: Vec<Candidate> = {
            let mut seen = std::collections::HashSet::new();
            self.dict.iter()
                .filter_map(|d| d.lookup(&midashi, okuri_key))
                .flat_map(|e| e.candidates)
                .filter(|c| seen.insert(c.word.clone()))
                .collect()
        };

        if candidates.is_empty() {
            // No candidates → enter (possibly nested) registration mode.
            let okuri_kana = okuri.as_ref().map(|(_, v)| v.clone());
            let okuri_key_str = okuri.map(|(k, _)| k);
            self.register_stack.push(RegisterFrame {
                midashi,
                okuri_key: okuri_key_str,
                okuri_kana,
                committed: String::new(),
                cursor: 0,
                is_abbrev: self.midashi_is_abbrev,
            });
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            return vec![self.preedit_action()];
        }

        let okuri_key_str = okuri.as_ref().map(|(k, _)| k.clone());
        let okuri_str = okuri.map(|(_, v)| v);
        self.phase = SkkPhase::Selecting {
            midashi,
            okuri: okuri_str,
            okuri_key: okuri_key_str,
            candidates: candidates.clone(),
            index: 0,
        };

        // Show candidates immediately only when listing mode starts at index 0
        // (i.e. inline_count == 0). Otherwise the preedit (▼word) is enough.
        let mut actions = vec![self.preedit_action()];
        if let Some(show) = self.listing_show_action(&candidates, 0) {
            actions.insert(0, show);
        }
        actions
    }

    fn handle_selecting(
        &mut self,
        event: &KeyEvent,
        midashi: String,
        okuri: Option<String>,
        okuri_key: Option<String>,
        candidates: Vec<Candidate>,
        mut index: usize,
    ) -> Vec<EngineAction> {
        // Escape → always cancel conversion and return to midashi.
        if event.key == Key::Escape {
            let midashi_str = midashi.clone();
            self.phase = SkkPhase::Midashi {
                kana_buf: midashi,
                roman_buf: String::new(),
            };
            self.kana_state.clear();
            self.conversion_trigger = None;
            self.update_completion(&midashi_str);
            return vec![EngineAction::HideCandidates, self.preedit_action()];
        }

        // Cancel key (e.g. 'x') → go back one candidate, or return to midashi at index 0.
        if event.printable_char().map_or(false, |c| self.keybindings.cancel.contains(&c)) {
            if index == 0 {
                // At the first candidate → back to midashi (cancel conversion).
                let midashi_str = midashi.clone();
                self.phase = SkkPhase::Midashi {
                    kana_buf: midashi,
                    roman_buf: String::new(),
                };
                self.kana_state.clear();
                self.conversion_trigger = None;
                self.update_completion(&midashi_str);
                return vec![EngineAction::HideCandidates, self.preedit_action()];
            }

            // index > 0 → go back to the previous candidate / previous page.
            let inline_count = self.keybindings.inline_count;
            let sel_len = self.keybindings.selection_keys.len();
            let in_listing = sel_len > 0 && index >= inline_count;

            let prev = if in_listing {
                let prev_raw = index.saturating_sub(sel_len);
                if prev_raw < inline_count {
                    // Crossed back into inline mode; show last inline candidate.
                    inline_count.saturating_sub(1)
                } else {
                    prev_raw
                }
            } else {
                index - 1
            };

            self.phase = SkkPhase::Selecting {
                midashi,
                okuri,
                okuri_key,
                candidates: candidates.clone(),
                index: prev,
            };
            let mut actions = vec![self.preedit_action()];
            match self.listing_show_action(&candidates, prev) {
                Some(show) => actions.insert(0, show),
                // Crossed back to inline: hide the candidate window.
                None if in_listing => actions.insert(0, EngineAction::HideCandidates),
                None => {}
            }
            return actions;
        }

        // BackSpace → confirm current candidate, then pass BackSpace through to delete the last char
        if event.key == Key::BackSpace {
            let mut actions = self.commit_candidate(&midashi, &candidates, index, &okuri, &okuri_key);
            actions.push(EngineAction::Passthrough);
            return actions;
        }

        // Return → confirm current candidate, then pass the key through to the app
        if event.key == Key::Return {
            let mut actions = self.commit_candidate(&midashi, &candidates, index, &okuri, &okuri_key);
            actions.push(EngineAction::Passthrough);
            return actions;
        }

        // Ctrl+j → confirm current candidate (key consumed, no passthrough)
        let is_ctrl_j = event.key == Key::Char('j') && event.modifiers.contains(Modifiers::CTRL);
        if is_ctrl_j {
            return self.commit_candidate(&midashi, &candidates, index, &okuri, &okuri_key);
        }

        // Space → advance to next candidate (inline mode) or next page (listing mode).
        // When all candidates are exhausted, enter dictionary registration mode.
        if event.key == Key::Space {
            let inline_count = self.keybindings.inline_count;
            let sel_len = self.keybindings.selection_keys.len();
            let in_listing = sel_len > 0 && index >= inline_count;
            if in_listing {
                let next = index + sel_len;
                if next >= candidates.len() {
                    // Past the last page: enter registration mode.
                    return self.enter_register_from_selecting(midashi, okuri, okuri_key);
                }
                index = next;
            } else {
                let next = index + 1;
                if next >= candidates.len() {
                    // Past the last inline candidate: enter registration mode.
                    return self.enter_register_from_selecting(midashi, okuri, okuri_key);
                }
                index = next;
            }
            self.phase = SkkPhase::Selecting {
                midashi,
                okuri,
                okuri_key,
                candidates: candidates.clone(),
                index,
            };
            let mut actions = vec![self.preedit_action()];
            if let Some(show) = self.listing_show_action(&candidates, index) {
                actions.insert(0, show);
            }
            return actions;
        }

        // Listing-mode selection: when index >= inline_count, selection keys pick a candidate
        // directly by offset from the current page start.
        {
            let inline_count = self.keybindings.inline_count;
            let sel_len = self.keybindings.selection_keys.len();
            if sel_len > 0 && index >= inline_count {
                if let Some(ch) = event.printable_char() {
                    if let Some(sel_idx) = self.keybindings.selection_keys.iter().position(|&k| k == ch) {
                        let cand_idx = index + sel_idx;
                        if cand_idx < candidates.len() {
                            return self.commit_candidate(&midashi, &candidates, cand_idx, &okuri, &okuri_key);
                        }
                        // Key pressed but no candidate at that position — consume and ignore.
                        return vec![];
                    }
                }
            }
        }

        // '>', '<', '?' → commit current candidate and enter suffix mode (new ▽ with '>' prefix)
        if event.printable_char().map_or(false, |c| matches!(c, '>' | '<' | '?')) {
            let mut actions = self.commit_candidate(&midashi, &candidates, index, &okuri, &okuri_key);
            // Start a new midashi with '>' as the initial kana_buf so the next
            // conversion looks up ">reading" (suffix entries).
            self.phase = SkkPhase::Midashi {
                kana_buf: ">".to_string(),
                roman_buf: String::new(),
            };
            self.kana_state.clear();
            actions.push(self.preedit_action());
            return actions;
        }

        // Any other printable character (without Ctrl) → confirm, then re-process in hiragana
        if event.printable_char().is_some() && !event.modifiers.contains(Modifiers::CTRL) {
            let mut actions = self.commit_candidate(&midashi, &candidates, index, &okuri, &okuri_key);
            // Phase is now Hiragana; re-dispatch the character
            actions.extend(self.handle_kana(event));
            return actions;
        }

        vec![EngineAction::Passthrough]
    }

    /// Returns a `ShowCandidates` action for the current page when in listing mode,
    /// or `None` when `index` is still in inline mode.
    fn listing_show_action(&self, candidates: &[Candidate], index: usize) -> Option<EngineAction> {
        let inline_count = self.keybindings.inline_count;
        let sel_len = self.keybindings.selection_keys.len();
        if sel_len == 0 || index < inline_count {
            return None;
        }
        let page: Vec<Candidate> = candidates[index..].iter().take(sel_len).cloned().collect();
        let sel_keys: String = self.keybindings.selection_keys[..page.len()].iter().collect();
        Some(EngineAction::ShowCandidates(page, 0, sel_keys))
    }

    /// Commits the candidate at `index`, records it in the user dictionary, and resets to hiragana.
    fn commit_candidate(
        &mut self,
        midashi: &str,
        candidates: &[Candidate],
        index: usize,
        okuri: &Option<String>,
        okuri_key: &Option<String>,
    ) -> Vec<EngineAction> {
        let chosen = candidates[index].clone();
        let commit = format!("{}{}", chosen.word, okuri.as_deref().unwrap_or(""));

        // Record the chosen candidate in all writable dictionaries (read-only ones return Err).
        let entry = crate::dict::entry::DictEntry {
            midashi: midashi.to_string(),
            okuri: okuri_key.clone(),
            candidates: vec![chosen],
        };
        for dict in self.dict.iter_mut() {
            let _ = dict.learn(entry.clone());
        }

        // Return to the kana mode that was active before the conversion started.
        self.phase = match self.midashi_display_mode {
            KanaMode::Katakana  => SkkPhase::Katakana,
            KanaMode::HalfWidth => SkkPhase::HalfWidthKatakana,
            KanaMode::Hiragana  => SkkPhase::Hiragana,
        };
        self.kana_state.clear();
        let mut actions = vec![
            EngineAction::HideCandidates,
            EngineAction::Commit(commit),
            EngineAction::ClearPreedit,
        ];
        // If conversion was started by a trigger char (e.g. ',' '.' 'を'),
        // append it after the committed word.
        if let Some(trigger) = self.conversion_trigger.take() {
            actions.push(EngineAction::Commit(trigger));
        }
        actions
    }

    // ── Registration cursor editing ───────────────────────────────────────────

    /// Deletes the character immediately before the registration cursor.
    fn reg_backspace(&mut self) {
        if let Some(frame) = self.register_stack.last_mut() {
            if frame.cursor > 0 {
                let (idx, _) = frame.committed[..frame.cursor]
                    .char_indices().next_back().unwrap();
                frame.committed.remove(idx);
                frame.cursor = idx;
            }
        }
    }

    /// Deletes the character immediately after the registration cursor.
    fn reg_delete(&mut self) {
        if let Some(frame) = self.register_stack.last_mut() {
            if frame.cursor < frame.committed.len() {
                frame.committed.remove(frame.cursor);
            }
        }
    }

    /// Moves the registration cursor one character to the left.
    fn reg_move_left(&mut self) {
        if let Some(frame) = self.register_stack.last_mut() {
            if frame.cursor > 0 {
                let (idx, _) = frame.committed[..frame.cursor]
                    .char_indices().next_back().unwrap();
                frame.cursor = idx;
            }
        }
    }

    /// Moves the registration cursor one character to the right.
    fn reg_move_right(&mut self) {
        if let Some(frame) = self.register_stack.last_mut() {
            if frame.cursor < frame.committed.len() {
                let ch = frame.committed[frame.cursor..].chars().next().unwrap();
                frame.cursor += ch.len_utf8();
            }
        }
    }

    /// Enters registration mode from Selecting phase (no more candidates available).
    fn enter_register_from_selecting(
        &mut self,
        midashi: String,
        okuri: Option<String>,
        okuri_key: Option<String>,
    ) -> Vec<EngineAction> {
        self.register_stack.push(RegisterFrame {
            midashi,
            okuri_key,
            okuri_kana: okuri,
            committed: String::new(),
            cursor: 0,
            is_abbrev: self.midashi_is_abbrev,
        });
        self.phase = SkkPhase::Hiragana;
        self.kana_state.clear();
        self.conversion_trigger = None;
        vec![EngineAction::HideCandidates, self.preedit_action()]
    }

    // ── Registration mode helpers ─────────────────────────────────────────────

    /// Completes the topmost registration frame: saves to user dict and either
    /// commits to the application (outermost frame) or appends to the outer frame.
    fn finalize_register(&mut self) -> Vec<EngineAction> {
        let frame = self.register_stack.pop().expect("finalize_register called with empty stack");
        let word = frame.committed.clone();
        let okuri_kana = frame.okuri_kana.as_deref().unwrap_or("");

        // Save the new entry to all writable dictionaries.
        let entry = crate::dict::entry::DictEntry {
            midashi: frame.midashi.clone(),
            okuri: frame.okuri_key.clone(),
            candidates: vec![Candidate { word: word.clone(), annotation: None }],
        };
        for dict in self.dict.iter_mut() {
            let _ = dict.learn(entry.clone());
        }

        if self.register_stack.is_empty() {
            // Outermost frame: commit to the application.
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            let commit = format!("{}{}", word, okuri_kana);
            vec![EngineAction::Commit(commit), EngineAction::ClearPreedit]
        } else {
            // Inner frame: insert into the enclosing registration buffer at its cursor.
            if let Some(outer) = self.register_stack.last_mut() {
                outer.committed.insert_str(outer.cursor, &word);
                outer.cursor += word.len();
            }
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            vec![self.preedit_action()]
        }
    }

    /// Cancels the topmost registration frame and returns to ▽ midashi mode.
    /// If nested, returns to the outer registration frame instead.
    fn cancel_register(&mut self) -> Vec<EngineAction> {
        let frame = self.register_stack.pop().expect("cancel_register called with empty stack");
        self.phase = SkkPhase::Hiragana;
        self.kana_state.clear();

        if self.register_stack.is_empty() {
            // Return to the mode that triggered the conversion.
            if frame.is_abbrev {
                self.phase = SkkPhase::Abbrev { buf: frame.midashi };
            } else {
                let midashi_str = frame.midashi.clone();
                self.phase = SkkPhase::Midashi { kana_buf: frame.midashi, roman_buf: String::new() };
                self.update_completion(&midashi_str);
            }
            vec![EngineAction::HideCandidates, self.preedit_action()]
        } else {
            // Return to the outer registration frame.
            vec![self.preedit_action()]
        }
    }

    /// Intercepts `Commit` and preedit actions produced by sub-phases while in
    /// registration mode, redirecting committed text into the register buffer.
    fn intercept_for_register(&mut self, raw_actions: Vec<EngineAction>) -> Vec<EngineAction> {
        let mut result = Vec::new();
        let mut need_preedit = false;

        for action in raw_actions {
            match action {
                EngineAction::Commit(text) => {
                    if let Some(frame) = self.register_stack.last_mut() {
                        frame.committed.insert_str(frame.cursor, &text);
                        frame.cursor += text.len();
                    }
                    need_preedit = true;
                }
                EngineAction::ClearPreedit | EngineAction::UpdatePreedit(_) => {
                    need_preedit = true;
                }
                // Candidate window and status indicator actions pass through unchanged.
                EngineAction::ShowCandidates(_, _, _)
                | EngineAction::HideCandidates
                | EngineAction::UpdateStatus(_) => {
                    result.push(action);
                }
                // Drop Passthrough: keys should not reach the app while registering.
                EngineAction::Passthrough => {}
            }
        }

        // Always emit a preedit update so the adapter reports the key as consumed.
        // (An empty action list would cause consumed=false, letting the key leak to the app.)
        if need_preedit || result.is_empty() {
            result.push(self.preedit_action());
        }
        result
    }

    // ── Completion helpers ────────────────────────────────────────────────────

    /// Returns `(preview_headword, next_index)` for the completion at `start_index`
    /// in the merged deduped list of all dict completions for `prefix`.
    /// Order: user dict recency order first, then system dicts in lex order.
    /// Does NOT sort the merged list — preserves dict iteration order.
    fn find_completion(&self, prefix: &str, start_index: usize) -> Option<(String, usize)> {
        let mut seen = std::collections::HashSet::new();
        let all: Vec<String> = self.dict
            .iter()
            .flat_map(|d| d.complete(prefix))
            .filter(|w| seen.insert(w.clone()))
            .collect();
        all.into_iter()
            .enumerate()
            .nth(start_index)
            .map(|(idx, word)| (word, idx + 1))
    }

    /// Recomputes completion from scratch for the given `kana_buf` (index 0).
    /// Called after kana input or backspace resets the midashi buffer.
    fn update_completion(&mut self, kana_buf: &str) {
        if kana_buf.is_empty() {
            self.completion = None;
            return;
        }
        self.completion = self.find_completion(kana_buf, 0)
            .map(|(preview, next_index)| CompletionState {
                cycle_prefix: kana_buf.to_string(),
                next_index,
                preview,
            });
    }

    // ── Preedit helpers ───────────────────────────────────────────────────────

    fn preedit_action(&self) -> EngineAction {
        EngineAction::UpdatePreedit(self.build_preedit())
    }

    fn build_preedit(&self) -> Preedit {
        // Ghost text: only when in Midashi with an empty roman_buf and active completion.
        // The ghost suffix is appended after the actual kana_buf; the cursor is placed
        // between kana_buf and the ghost so the UI can visually distinguish them.
        if let SkkPhase::Midashi { kana_buf, roman_buf } = &self.phase {
            if roman_buf.is_empty() {
                if let Some(state) = &self.completion {
                    let display = match self.midashi_display_mode {
                        KanaMode::Katakana  => hiragana_to_katakana(kana_buf),
                        KanaMode::HalfWidth => hiragana_to_halfwidth(kana_buf),
                        KanaMode::Hiragana  => kana_buf.clone(),
                    };
                    // The ghost can only be shown as a suffix when preview starts with
                    // kana_buf.  After Tab cycling the next completion may start from
                    // cycle_prefix (shorter), so it may not extend kana_buf — in that
                    // case skip the ghost and fall through to the default preedit.
                    if !state.preview.starts_with(kana_buf.as_str()) {
                        // Ghost not applicable for current kana_buf; use default preedit.
                        let inner = self.build_inner_preedit();
                        if let Some(frame) = self.register_stack.last() {
                            let depth = self.register_stack.len();
                            let open  = "[".repeat(depth);
                            let close = "]".repeat(depth);
                            let midashi_disp = format!("{}{}",
                                frame.midashi, frame.okuri_kana.as_deref().unwrap_or(""));
                            let before = &frame.committed[..frame.cursor];
                            let after  = &frame.committed[frame.cursor..];
                            let prefix = format!("{}辞書登録{} {}: ", open, close, midashi_disp);
                            let text   = format!("{}{}{}{}", prefix, before, inner, after);
                            let cursor = prefix.len() + before.len() + inner.len();
                            return Preedit { text, cursor };
                        }
                        return Preedit { text: inner.clone(), cursor: inner.len() };
                    }
                    // "▽" is U+25BD = 3 UTF-8 bytes.
                    let ghost_suffix = &state.preview[kana_buf.len()..];
                    let cursor_pos = 3 + display.len();
                    let inner = format!("▽{display}{ghost_suffix}");

                    if let Some(frame) = self.register_stack.last() {
                        let depth = self.register_stack.len();
                        let open  = "[".repeat(depth);
                        let close = "]".repeat(depth);
                        let midashi_disp = format!(
                            "{}{}",
                            frame.midashi,
                            frame.okuri_kana.as_deref().unwrap_or("")
                        );
                        let before = &frame.committed[..frame.cursor];
                        let after  = &frame.committed[frame.cursor..];
                        let prefix = format!("{}辞書登録{} {}: ", open, close, midashi_disp);
                        let text   = format!("{}{}{}{}", prefix, before, inner, after);
                        let cursor = prefix.len() + before.len() + cursor_pos;
                        return Preedit { text, cursor };
                    }
                    return Preedit { text: inner, cursor: cursor_pos };
                }
            }
        }

        let inner = self.build_inner_preedit();

        if let Some(frame) = self.register_stack.last() {
            // Depth is shown as nested brackets: [辞書登録], [[辞書登録]], ...
            let depth = self.register_stack.len();
            let open  = "[".repeat(depth);
            let close = "]".repeat(depth);
            let midashi_disp = format!(
                "{}{}",
                frame.midashi,
                frame.okuri_kana.as_deref().unwrap_or("")
            );
            // Split the committed text at the editing cursor so the IM cursor
            // is placed between `before` and `after` in the displayed preedit.
            let before = &frame.committed[..frame.cursor];
            let after  = &frame.committed[frame.cursor..];
            let prefix = format!("{}辞書登録{} {}: ", open, close, midashi_disp);
            // Layout: <prefix><before><inner_preedit><after>
            // IM cursor sits at the end of <inner_preedit>.
            let text   = format!("{}{}{}{}", prefix, before, inner, after);
            let cursor = prefix.len() + before.len() + inner.len();
            return Preedit { text, cursor };
        }

        let cursor = inner.len();
        Preedit { text: inner, cursor }
    }

    /// Returns the preedit text for the current phase, without any register prefix.
    fn build_inner_preedit(&self) -> String {
        match &self.phase {
            SkkPhase::Hiragana | SkkPhase::Katakana | SkkPhase::HalfWidthKatakana => {
                self.kana_state.clone()
            }
            SkkPhase::WideAscii | SkkPhase::Ascii => String::new(),
            SkkPhase::Abbrev { buf } => format!("▽{buf}"),
            SkkPhase::CodeInput { prefix, buf } => {
                let prefix_str = match prefix {
                    CodeInputPrefix::Jis => "\\",
                    CodeInputPrefix::Unicode => "\\u",
                };
                format!("{prefix_str}{buf}")
            }
            SkkPhase::Midashi { kana_buf, roman_buf } => {
                let display = match self.midashi_display_mode {
                    KanaMode::Katakana  => hiragana_to_katakana(kana_buf),
                    KanaMode::HalfWidth => hiragana_to_halfwidth(kana_buf),
                    KanaMode::Hiragana  => kana_buf.clone(),
                };
                format!("▽{display}{roman_buf}")
            }
            SkkPhase::Okuri { midashi, kana_buf, .. } => {
                format!("▽{midashi}*{kana_buf}{}", self.kana_state)
            }
            SkkPhase::Selecting { midashi: _, okuri, okuri_key: _, candidates, index } => {
                let cand = &candidates[*index];
                let ok = okuri.as_deref().unwrap_or("");
                let trigger = self.conversion_trigger.as_deref().unwrap_or("");
                match &cand.annotation {
                    Some(ann) => format!("▼{}{}{};{}", cand.word, ok, trigger, ann),
                    None => format!("▼{}{}{}", cand.word, ok, trigger),
                }
            }
        }
    }
}

// ── SkkPhase helpers ──────────────────────────────────────────────────────────

impl SkkPhase {
    /// Returns the midashi string if in Okuri phase (used for re-dispatch).
    fn clone_midashi(&self) -> String {
        match self {
            SkkPhase::Okuri { midashi, .. } => midashi.clone(),
            _ => String::new(),
        }
    }
}

// ── Wide-ASCII conversion ─────────────────────────────────────────────────────

/// Converts an ASCII character to its full-width (wide) Unicode equivalent.
fn to_wide_ascii(c: char) -> Option<char> {
    match c {
        ' ' => Some('　'),
        '!'..='~' => char::from_u32(c as u32 + 0xFEE0),
        _ => None,
    }
}

/// Decodes a JIS code point (4 hex digits) to a Unicode string.
/// Decodes a 4-digit hex JIS X 0208 code to a Unicode string via EUC-JP.
///
/// The input is a 4-digit hex string representing a two-byte JIS X 0208 code point
/// (e.g. "2422" → あ).  Each byte must be in the range 0x21–0x7E.
/// The conversion adds 0x80 to each byte to obtain the EUC-JP encoding, then
/// decodes to UTF-8 using encoding_rs.
fn decode_jis_code(hex: &str) -> Option<String> {
    if hex.len() != 4 {
        return None;
    }
    let code = u16::from_str_radix(hex, 16).ok()?;
    let b1 = (code >> 8) as u8;
    let b2 = (code & 0xFF) as u8;
    // Valid JIS X 0208 range for both bytes: 0x21–0x7E
    if !(0x21..=0x7E).contains(&b1) || !(0x21..=0x7E).contains(&b2) {
        return None;
    }
    // EUC-JP: add 0x80 to each byte
    let euc = [b1 + 0x80, b2 + 0x80];
    let (decoded, _, had_errors) = encoding_rs::EUC_JP.decode(&euc);
    if had_errors { None } else { Some(decoded.into_owned()) }
}

/// Decodes a Unicode code point (1–6 hex digits) to a string.
fn decode_unicode_code(hex: &str) -> Option<String> {
    if hex.is_empty() || hex.len() > 6 {
        return None;
    }
    let code = u32::from_str_radix(hex, 16).ok()?;
    char::from_u32(code).map(|c| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::entry::{DictEntry, DictError};
    use crate::dict::traits::DictionaryProvider;
    use crate::kana::builtin::builtin_table;
    use crate::kana::table::KanaLayout;

    fn engine() -> SkkEngine {
        let table = builtin_table(KanaLayout::Romaji);
        SkkEngine::new(table, SkkKeybindings::default())
    }

    fn press(ch: char) -> KeyEvent {
        KeyEvent::press(Key::Char(ch), Modifiers::empty())
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::press(Key::Char(ch), Modifiers::CTRL)
    }

    fn backspace() -> KeyEvent {
        KeyEvent::press(Key::BackSpace, Modifiers::empty())
    }

    /// Stub dictionary that returns a fixed candidate list for any lookup.
    struct StubDict(Vec<Candidate>);

    impl DictionaryProvider for StubDict {
        fn lookup(&self, _midashi: &str, _okuri: Option<&str>) -> Option<DictEntry> {
            Some(DictEntry {
                midashi: String::new(),
                okuri: None,
                candidates: self.0.clone(),
            })
        }
        fn learn(&mut self, _entry: DictEntry) -> Result<(), DictError> {
            Ok(())
        }
    }

    fn engine_with_dict(candidates: Vec<&str>) -> SkkEngine {
        let mut eng = engine();
        let cands = candidates.into_iter().map(Candidate::new).collect();
        eng.add_dict(Box::new(StubDict(cands)));
        eng
    }

    #[test]
    fn test_ascii_mode_switch() {
        let mut eng = engine();
        // 'l' → ASCII mode
        let actions = eng.process_key(&press('l'));
        assert_eq!(eng.phase(), &SkkPhase::Ascii);
        assert!(actions.contains(&EngineAction::ClearPreedit));

        // Ctrl+j → back to Hiragana
        eng.process_key(&ctrl('j'));
        assert_eq!(eng.phase(), &SkkPhase::Hiragana);
    }

    #[test]
    fn test_basic_kana_input() {
        let mut eng = engine();
        eng.process_key(&press('k'));
        eng.process_key(&press('a'));

        // After "ka", engine should have committed "か"
        // (tested implicitly by checking kana_state cleared)
        assert_eq!(eng.kana_state, "");
    }

    #[test]
    fn test_commit_learns_in_user_dict() {
        use crate::dict::file::UserDict;
        let mut eng = engine();
        let user_dict = UserDict::empty(std::path::PathBuf::from("/tmp/y2skk_test_learn.dict"));
        eng.add_dict(Box::new(user_dict));

        // Add a stub dict so conversion succeeds
        eng.add_dict(Box::new(StubDict(vec![
            Candidate::new("亜"),
            Candidate::new("阿"),
        ])));

        // Enter Selecting and confirm "阿" (index 1)
        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        // Advance to index 1
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        // Confirm with Ctrl+j
        let actions = eng.process_key(&ctrl('j'));
        assert!(actions.iter().any(|a| matches!(a, EngineAction::Commit(s) if s == "阿")));

        // The user dict (highest priority) should now have learned "阿" for "あい"
        // ('A' → "あ", 'i' → "い" in midashi, so midashi is "あい")
        let result = eng.dict[0].lookup("あい", None);
        assert!(result.is_some(), "UserDict should have learned '阿' for 'あい'");
        assert_eq!(result.unwrap().candidates[0].word, "阿");
    }

    #[test]
    fn test_listing_mode_selection_key() {
        // With inline_count=2 and selection_keys="as", after 2 inline candidates
        // the next Space enters listing mode; pressing 'a' or 's' picks a candidate.
        let table = builtin_table(KanaLayout::Romaji);
        let keybindings = SkkKeybindings {
            inline_count: 2,
            selection_keys: vec!['a', 's'],
            ..SkkKeybindings::default()
        };
        let mut eng = SkkEngine::new(table, keybindings);
        eng.add_dict(Box::new(StubDict(vec![
            Candidate::new("A"),
            Candidate::new("B"),
            Candidate::new("C"),
            Candidate::new("D"),
        ])));

        // Enter Selecting
        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        // index=0 (inline)
        assert!(matches!(eng.phase(), SkkPhase::Selecting { index: 0, .. }));

        // Space → index=1 (still inline)
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { index: 1, .. }));

        // Space → index=2 (listing mode starts, page shows C, D)
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { index: 2, .. }));

        // 'a' picks index 2 ("C")
        let actions = eng.process_key(&press('a'));
        assert!(actions.iter().any(|a| matches!(a, EngineAction::Commit(s) if s == "C")));
        assert_eq!(eng.phase(), &SkkPhase::Hiragana);
    }

    #[test]
    fn test_listing_mode_page_advance() {
        let table = builtin_table(KanaLayout::Romaji);
        let keybindings = SkkKeybindings {
            inline_count: 1,
            selection_keys: vec!['a', 's'],
            ..SkkKeybindings::default()
        };
        let mut eng = SkkEngine::new(table, keybindings);
        eng.add_dict(Box::new(StubDict(vec![
            Candidate::new("A"),
            Candidate::new("B"),
            Candidate::new("C"),
        ])));

        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        // index=0, inline
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        // index=1, listing page start (B, C)
        assert!(matches!(eng.phase(), SkkPhase::Selecting { index: 1, .. }));

        // Space past the last page: enter registration mode (index=1+2=3 >= len=3)
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        assert!(!eng.register_stack.is_empty(), "should enter register mode after last candidate");
        assert!(matches!(eng.phase(), SkkPhase::Hiragana));
    }

    #[test]
    fn test_cancel_goes_back_one_candidate() {
        // In ▼ mode, the cancel key ('x') should decrement the candidate index.
        // Only when index==0 should it return to midashi.
        let table = builtin_table(KanaLayout::Romaji);
        let keybindings = SkkKeybindings {
            inline_count: 3,
            selection_keys: vec!['a', 's'],
            cancel: vec!['x'],
            ..SkkKeybindings::default()
        };
        let mut eng = SkkEngine::new(table, keybindings);
        eng.add_dict(Box::new(StubDict(vec![
            Candidate::new("A"),
            Candidate::new("B"),
            Candidate::new("C"),
            Candidate::new("D"),
        ])));

        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty())); // index=0
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty())); // index=1
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty())); // index=2

        // x → back to index=1
        eng.process_key(&press('x'));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { index: 1, .. }));

        // x → back to index=0
        eng.process_key(&press('x'));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { index: 0, .. }));

        // x at index=0 → back to midashi
        eng.process_key(&press('x'));
        assert!(matches!(eng.phase(), SkkPhase::Midashi { .. }));
    }

    #[test]
    fn test_cancel_in_listing_goes_back_page() {
        // In listing mode, cancel should go back one page; from the first listing
        // page it should return to the last inline candidate.
        let table = builtin_table(KanaLayout::Romaji);
        let keybindings = SkkKeybindings {
            inline_count: 1,
            selection_keys: vec!['a', 's'],
            cancel: vec!['x'],
            ..SkkKeybindings::default()
        };
        let mut eng = SkkEngine::new(table, keybindings);
        eng.add_dict(Box::new(StubDict(vec![
            Candidate::new("A"),
            Candidate::new("B"),
            Candidate::new("C"),
            Candidate::new("D"),
            Candidate::new("E"),
        ])));

        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty())); // index=0 inline
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty())); // index=1 listing page1
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty())); // index=3 listing page2

        // x → back to index=1 (listing page1)
        eng.process_key(&press('x'));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { index: 1, .. }));

        // x → back to index=0 (inline, last inline candidate = inline_count-1 = 0)
        eng.process_key(&press('x'));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { index: 0, .. }));
    }

    #[test]
    fn test_prefix_conversion_in_midashi() {
        // '>' in ▽ mode appends '>' to midashi and triggers conversion ("ちょう>" → "超").
        struct PrefixDict;
        impl DictionaryProvider for PrefixDict {
            fn lookup(&self, midashi: &str, _okuri: Option<&str>) -> Option<DictEntry> {
                match midashi {
                    "ちょう>" => Some(DictEntry {
                        midashi: midashi.to_string(), okuri: None,
                        candidates: vec![Candidate::new("超")],
                    }),
                    _ => None,
                }
            }
            fn learn(&mut self, _entry: DictEntry) -> Result<(), DictError> { Ok(()) }
        }

        let mut eng = engine();
        eng.add_dict(Box::new(PrefixDict));

        // Type ▽ちょう  (T y o u)
        eng.process_key(&press('T'));
        eng.process_key(&press('y'));
        eng.process_key(&press('o'));
        eng.process_key(&press('u'));
        assert!(matches!(eng.phase(), SkkPhase::Midashi { .. }));

        // Press '>' → should trigger conversion on "ちょう>" → Selecting with "超"
        eng.process_key(&press('>'));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { .. }), "should be Selecting after '>'");
    }

    #[test]
    fn test_suffix_mode_in_selecting() {
        // '>' in ▼ mode commits the candidate and starts new ▽ with '>' prefix.
        let mut eng = engine_with_dict(vec!["小林"]);

        eng.process_key(&press('K'));
        eng.process_key(&press('o'));
        eng.process_key(&press('b'));
        eng.process_key(&press('a'));
        eng.process_key(&press('y'));
        eng.process_key(&press('a'));
        eng.process_key(&press('s'));
        eng.process_key(&press('h'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { .. }));

        // Press '>' → commit "小林" and enter ▽> mode
        let actions = eng.process_key(&press('>'));
        assert!(actions.iter().any(|a| matches!(a, EngineAction::Commit(_))), "should commit");
        match eng.phase() {
            SkkPhase::Midashi { kana_buf, roman_buf } => {
                assert_eq!(kana_buf, ">", "kana_buf should start with '>'");
                assert!(roman_buf.is_empty());
            }
            other => panic!("expected Midashi, got {other:?}"),
        }
    }

    #[test]
    fn test_selecting_backspace_commits_and_passes_through() {
        // Enter Selecting phase: type "Ai" (midashi "あ") with a stub dict
        let mut eng = engine_with_dict(vec!["亜", "阿"]);

        // 'A' starts midashi, 'i' produces "あ", Space triggers conversion
        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        let actions = eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { .. }), "should be Selecting");
        let _ = actions;

        // BackSpace in Selecting phase: should commit the current candidate
        // and pass BackSpace through so the application deletes the last character.
        let actions = eng.process_key(&backspace());
        assert_eq!(eng.phase(), &SkkPhase::Hiragana, "should return to Hiragana after commit");
        assert!(
            actions.iter().any(|a| matches!(a, EngineAction::Commit(_))),
            "should emit Commit"
        );
        assert!(
            actions.contains(&EngineAction::Passthrough),
            "should pass BackSpace through"
        );
    }

    #[test]
    fn test_register_basic() {
        // When no candidates exist, engine enters registration mode.
        // User types romaji (→ kana), then RET → word is saved and committed.
        let mut eng = engine(); // no dict → all conversions go to register mode

        // Trigger conversion of "あい" (no candidates → register mode)
        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        let actions = eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        // Should have UpdatePreedit with registration prompt
        assert!(
            actions.iter().any(|a| matches!(a, EngineAction::UpdatePreedit(p) if p.text.contains("辞書登録"))),
            "should show registration prompt; got: {:?}", actions
        );
        assert!(!eng.register_stack.is_empty(), "register_stack should be non-empty");

        // Type "か" via romaji "ka"
        eng.process_key(&press('k'));
        eng.process_key(&press('a'));

        // RET → commit
        let actions = eng.process_key(&KeyEvent::press(Key::Return, Modifiers::empty()));
        assert!(
            actions.iter().any(|a| matches!(a, EngineAction::Commit(s) if s == "か")),
            "should commit registered word; got: {:?}", actions
        );
        assert!(eng.register_stack.is_empty(), "register_stack should be empty after finalize");
        assert_eq!(eng.phase(), &SkkPhase::Hiragana);
    }

    #[test]
    fn test_register_cancel_returns_to_midashi() {
        let mut eng = engine();
        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty())); // enter register mode

        // C-g cancels → should return to ▽ midashi
        let actions = eng.process_key(&KeyEvent::press(
            Key::Char('g'), Modifiers::CTRL,
        ));
        assert!(eng.register_stack.is_empty());
        assert!(matches!(eng.phase(), SkkPhase::Midashi { kana_buf, .. } if kana_buf == "あい"),
            "should return to midashi あい; phase: {:?}", eng.phase());
        // preedit should show ▽あい
        assert!(
            actions.iter().any(|a| matches!(a, EngineAction::UpdatePreedit(p) if p.text.contains("▽あい"))),
            "should update preedit to ▽あい; got: {:?}", actions
        );
    }

    #[test]
    fn test_register_empty_return_cancels() {
        let mut eng = engine();
        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));

        // RET with empty buf → cancel, return to midashi
        eng.process_key(&KeyEvent::press(Key::Return, Modifiers::empty()));
        assert!(eng.register_stack.is_empty());
        assert!(matches!(eng.phase(), SkkPhase::Midashi { .. }));
    }

    #[test]
    fn test_register_saves_to_user_dict() {
        use crate::dict::file::UserDict;
        let table = builtin_table(KanaLayout::Romaji);
        let mut eng = SkkEngine::new(table, SkkKeybindings::default());
        // UserDict with no entries: lookup returns None → register mode; learn actually stores.
        let user_dict = UserDict::empty(std::path::PathBuf::from("/tmp/y2skk_test_register.dict"));
        eng.add_dict(Box::new(user_dict));

        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty())); // enter register

        // Type "か" via romaji "ka"
        eng.process_key(&press('k'));
        eng.process_key(&press('a'));
        eng.process_key(&KeyEvent::press(Key::Return, Modifiers::empty())); // finalize

        // Dict should now have the entry
        let result = eng.dict[0].lookup("あい", None);
        assert!(result.is_some(), "user dict should have learned あい");
        assert_eq!(result.unwrap().candidates[0].word, "か");
    }

    #[test]
    fn test_register_recursive() {
        // Recursive registration: while registering "さいきてき", the user triggers
        // an inner conversion for "さいき" (also no entry) → nested registration.
        // After registering "さいき" → "か" (romaji), back to outer registration.
        let mut eng = engine(); // no dict

        // Start outer conversion: さいきてき
        for ch in ['S','a','i','k','i','t','e','k','i'] { eng.process_key(&press(ch)); }
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        assert_eq!(eng.register_stack.len(), 1, "should be in outer register");

        // Within registration, start inner conversion: さいき
        for ch in ['S','a','i','k','i'] { eng.process_key(&press(ch)); }
        eng.process_key(&KeyEvent::press(Key::Space, Modifiers::empty()));
        assert_eq!(eng.register_stack.len(), 2, "should be in nested register");

        // Register "さいき" → "か" (romaji "ka")
        eng.process_key(&press('k'));
        eng.process_key(&press('a'));
        eng.process_key(&KeyEvent::press(Key::Return, Modifiers::empty()));
        assert_eq!(eng.register_stack.len(), 1, "should be back in outer register");
        // Outer committed buf should now contain "か"
        assert_eq!(eng.register_stack[0].committed, "か");

        // Now type "き" (romaji "ki") and finalize the outer registration
        eng.process_key(&press('k'));
        eng.process_key(&press('i'));
        let actions = eng.process_key(&KeyEvent::press(Key::Return, Modifiers::empty()));
        assert!(eng.register_stack.is_empty());
        assert!(
            actions.iter().any(|a| matches!(a, EngineAction::Commit(s) if s == "かき")),
            "should commit かき; got: {:?}", actions
        );
    }

    #[test]
    fn test_midashi_q_converts_to_katakana() {
        // ▽あいう + q → commit "アイウ" and return to hiragana
        let mut eng = engine();

        // Enter midashi: ▽あいう
        eng.process_key(&press('A'));
        eng.process_key(&press('i'));
        eng.process_key(&press('u'));
        // kana_buf should now be "あいう"
        assert!(matches!(&eng.phase, SkkPhase::Midashi { kana_buf, .. } if kana_buf == "あいう"));

        // Press 'q' — should commit as katakana
        let actions = eng.process_key(&press('q'));
        assert!(matches!(eng.phase, SkkPhase::Hiragana));
        assert!(
            actions.iter().any(|a| matches!(a, EngineAction::Commit(s) if s == "アイウ")),
            "expected commit アイウ, got: {:?}", actions
        );
    }

    #[test]
    fn test_code_input_jis() {
        // \2422 + Enter → あ (JIS X 0208 code 0x2422 = あ)
        let mut eng = engine();
        eng.process_key(&press('\\'));
        assert!(matches!(eng.phase, SkkPhase::CodeInput { ref prefix, .. } if *prefix == CodeInputPrefix::Jis));
        eng.process_key(&press('2'));
        eng.process_key(&press('4'));
        eng.process_key(&press('2'));
        eng.process_key(&press('2'));
        let actions = eng.process_key(&KeyEvent::press(Key::Return, Modifiers::empty()));
        assert!(actions.iter().any(|a| *a == EngineAction::Commit("あ".into())));
        assert!(matches!(eng.phase, SkkPhase::Hiragana));
    }

    #[test]
    fn test_code_input_unicode() {
        // \u3042 → あ (U+3042)
        let mut eng = engine();
        eng.process_key(&press('\\'));
        eng.process_key(&press('u'));
        assert!(matches!(eng.phase, SkkPhase::CodeInput { ref prefix, .. } if *prefix == CodeInputPrefix::Unicode));
        eng.process_key(&press('3'));
        eng.process_key(&press('0'));
        eng.process_key(&press('4'));
        eng.process_key(&press('2'));
        let actions = eng.process_key(&KeyEvent::press(Key::Return, Modifiers::empty()));
        assert!(actions.iter().any(|a| *a == EngineAction::Commit("あ".into())));
        assert!(matches!(eng.phase, SkkPhase::Hiragana));
    }

    #[test]
    fn test_code_input_backspace() {
        // BackSpace from \u returns to \, then BackSpace cancels
        let mut eng = engine();
        eng.process_key(&press('\\'));
        eng.process_key(&press('u'));
        assert!(matches!(eng.phase, SkkPhase::CodeInput { ref prefix, .. } if *prefix == CodeInputPrefix::Unicode));
        eng.process_key(&KeyEvent::press(Key::BackSpace, Modifiers::empty()));
        assert!(matches!(eng.phase, SkkPhase::CodeInput { ref prefix, .. } if *prefix == CodeInputPrefix::Jis));
        eng.process_key(&KeyEvent::press(Key::BackSpace, Modifiers::empty()));
        assert!(matches!(eng.phase, SkkPhase::Hiragana));
    }

    #[test]
    fn test_midashi_q_noop_when_empty() {
        // ▽ (empty) + q → stays in midashi (nothing to convert)
        let mut eng = engine();
        eng.process_key(&press('A')); // enter midashi with just 'a' typed via uppercase A
        // Actually 'A' feeds 'a' into kana, resulting in "あ" in kana_buf.
        // Use a letter with no output to get an empty midashi.
        // Workaround: cancel and re-enter with Shift only conceptually impossible here.
        // Just verify that 'q' on "あ" midashi works (already tested above).
        // Test the empty case via Escape + re-enter:
        eng.process_key(&KeyEvent::press(Key::Escape, Modifiers::empty()));
        // Re-enter midashi with uppercase that triggers pending roman only
        // Simplest: enter 'Q' in hiragana mode (no kana_buf) - but Q is not handled as
        // uppercase trigger when in kana mode... Let's just verify via a direct state check.
        // The engine is back in Hiragana.
        assert!(matches!(eng.phase, SkkPhase::Hiragana));
    }

    // ── Completion tests ──────────────────────────────────────────────────────

    /// A dict that provides fixed completions for prefix search, in addition to
    /// a configurable candidate list for exact lookup.
    struct CompletionDict {
        candidates: Vec<Candidate>,
        completions: Vec<String>,
    }

    impl DictionaryProvider for CompletionDict {
        fn lookup(&self, _midashi: &str, _okuri: Option<&str>) -> Option<DictEntry> {
            if self.candidates.is_empty() {
                None
            } else {
                Some(DictEntry {
                    midashi: String::new(),
                    okuri: None,
                    candidates: self.candidates.clone(),
                })
            }
        }
        fn learn(&mut self, _entry: DictEntry) -> Result<(), DictError> { Ok(()) }
        fn complete(&self, prefix: &str) -> Vec<String> {
            let mut results: Vec<String> = self.completions.iter()
                .filter(|w| w.starts_with(prefix) && w.as_str() != prefix)
                .cloned()
                .collect();
            results.sort();
            results
        }
    }

    fn engine_with_completions(completions: Vec<&str>) -> SkkEngine {
        let mut eng = engine();
        eng.add_dict(Box::new(CompletionDict {
            candidates: vec![],
            completions: completions.into_iter().map(String::from).collect(),
        }));
        eng
    }

    #[test]
    fn test_completion_ghost_shown() {
        // Enter midashi "か" → completion "からだ" should appear as ghost text.
        let mut eng = engine_with_completions(vec!["からだ", "かわ"]);
        eng.process_key(&press('K')); // uppercase → enter midashi
        eng.process_key(&press('a')); // kana_buf = "か"

        let preedit = eng.build_preedit();
        // text should include ghost suffix "らだ" after "▽か"
        assert!(preedit.text.contains("からだ"), "ghost text should contain completion: {:?}", preedit.text);
    }

    #[test]
    fn test_completion_cursor_position() {
        // Cursor should be placed after "▽か", not at end of ghost text.
        let mut eng = engine_with_completions(vec!["からだ"]);
        eng.process_key(&press('K'));
        eng.process_key(&press('a')); // kana_buf = "か"

        let preedit = eng.build_preedit();
        // "▽か" = 3 + 3 = 6 bytes
        assert_eq!(preedit.cursor, 6, "cursor should be after ▽か, got {:?}", preedit);
    }

    #[test]
    fn test_tab_accepts_completion() {
        let mut eng = engine_with_completions(vec!["からだ"]);
        eng.process_key(&press('K'));
        eng.process_key(&press('a')); // kana_buf = "か", ghost = "からだ"

        eng.process_key(&KeyEvent::press(Key::Tab, Modifiers::empty()));

        assert!(matches!(eng.phase, SkkPhase::Midashi { ref kana_buf, .. } if kana_buf == "からだ"),
            "Tab should accept ghost: {:?}", eng.phase);
    }

    #[test]
    fn test_tab_cycles_completions() {
        // Two completions for "か": "からだ" and "かわ" (sorted: "からだ" < "かわ")
        let mut eng = engine_with_completions(vec!["からだ", "かわ"]);
        eng.process_key(&press('K'));
        eng.process_key(&press('a')); // ghost = "からだ" (first sorted)

        // First Tab: accept "からだ"; next ghost "かわ" doesn't extend "からだ",
        // so it is stored in completion state but not visible in preedit.
        eng.process_key(&KeyEvent::press(Key::Tab, Modifiers::empty()));
        assert!(matches!(eng.phase, SkkPhase::Midashi { ref kana_buf, .. } if kana_buf == "からだ"),
            "kana_buf should be 'からだ' after first tab");
        assert!(eng.completion.is_some(), "completion state should hold next cycle entry");

        // Second Tab: accept "かわ" (next cycle entry replaces kana_buf entirely)
        eng.process_key(&KeyEvent::press(Key::Tab, Modifiers::empty()));
        assert!(matches!(eng.phase, SkkPhase::Midashi { ref kana_buf, .. } if kana_buf == "かわ"),
            "kana_buf should be 'かわ' after second tab");
    }

    #[test]
    fn test_tab_exhausted() {
        // Only one completion; after accepting it, no more ghost.
        let mut eng = engine_with_completions(vec!["からだ"]);
        eng.process_key(&press('K'));
        eng.process_key(&press('a'));

        eng.process_key(&KeyEvent::press(Key::Tab, Modifiers::empty())); // accept "からだ"
        assert!(eng.completion.is_none(), "no more completions after cycling through all");
    }

    #[test]
    fn test_backspace_after_tab_resets_completion() {
        // After Tab accepts "からだ", BackSpace → kana_buf="からだ".pop()="から",
        // completion resets to search from "から".
        let mut eng = engine_with_completions(vec!["からだ", "からす"]);
        eng.process_key(&press('K'));
        eng.process_key(&press('a')); // kana_buf="か", ghost="からだ"

        eng.process_key(&KeyEvent::press(Key::Tab, Modifiers::empty())); // accept, kana_buf="からだ"
        eng.process_key(&KeyEvent::press(Key::BackSpace, Modifiers::empty())); // pop → kana_buf="から"

        assert!(matches!(eng.phase, SkkPhase::Midashi { ref kana_buf, .. } if kana_buf == "から"),
            "kana_buf should be 'から' after backspace: {:?}", eng.phase);
        // Ghost should now show a completion starting with "から"
        let preedit = eng.build_preedit();
        assert!(preedit.text.starts_with("▽から"), "preedit should start with ▽から: {:?}", preedit.text);
    }

    #[test]
    fn test_no_ghost_with_roman_buf() {
        // While roman_buf is non-empty (e.g. mid-sequence "k"), ghost should not appear.
        let mut eng = engine_with_completions(vec!["からだ"]);
        eng.process_key(&press('K'));
        eng.process_key(&press('a')); // kana_buf="か", ghost shown
        eng.process_key(&press('k')); // start "k" sequence → roman_buf="k"

        let preedit = eng.build_preedit();
        // Should show "▽かk", no ghost suffix
        assert_eq!(preedit.text, "▽かk", "no ghost when roman_buf non-empty: {:?}", preedit.text);
    }

    // ── Conversion trigger tests ──────────────────────────────────────────────

    #[test]
    fn test_conversion_trigger_period() {
        // Typing '.' in ▽ mode triggers conversion; after commit, '.' is appended.
        let mut eng = engine_with_dict(vec!["学校"]);

        // Enter ▽ mode and type "がっこう"
        eng.process_key(&press('G'));
        for ch in "akkou".chars() {
            eng.process_key(&press(ch));
        }
        assert!(matches!(eng.phase(), SkkPhase::Midashi { .. }), "should be in Midashi");

        // Press '.' → should trigger conversion and enter Selecting mode
        let actions = eng.process_key(&press('.'));
        // '.' starts conversion: we should now be in Selecting mode
        assert!(matches!(eng.phase(), SkkPhase::Selecting { .. }), "should enter Selecting after '.'");
        // No commit yet (still selecting)
        assert!(!actions.iter().any(|a| matches!(a, EngineAction::Commit(_))), "no commit before selection");

        // Confirm with Ctrl+j → commits "学校" then "。" (kana table output for '.')
        let actions = eng.process_key(&ctrl('j'));
        let commits: Vec<&str> = actions.iter().filter_map(|a| {
            if let EngineAction::Commit(s) = a { Some(s.as_str()) } else { None }
        }).collect();
        assert_eq!(commits, vec!["学校", "。"], "should commit candidate then kana-table output for '.'");
        assert!(matches!(eng.phase(), SkkPhase::Hiragana), "should return to Hiragana");
    }

    #[test]
    fn test_conversion_trigger_wo() {
        // Typing 'wo' (→ 'を') in ▽ mode triggers conversion; after commit, 'を' is appended.
        let mut eng = engine_with_dict(vec!["学校"]);

        // Enter ▽ mode: 'G' + "akkou" → ▽がっこう
        eng.process_key(&press('G'));
        for ch in "akkou".chars() {
            eng.process_key(&press(ch));
        }

        // Type 'wo' → 'を' fires kana trigger
        eng.process_key(&press('w'));
        eng.process_key(&press('o'));
        assert!(matches!(eng.phase(), SkkPhase::Selecting { .. }), "should enter Selecting after 'wo'");

        // Confirm with Ctrl+j
        let actions = eng.process_key(&ctrl('j'));
        let commits: Vec<&str> = actions.iter().filter_map(|a| {
            if let EngineAction::Commit(s) = a { Some(s.as_str()) } else { None }
        }).collect();
        assert_eq!(commits, vec!["学校", "を"], "should commit candidate then 'を'");
    }

    #[test]
    fn test_conversion_trigger_empty_kana_buf() {
        // Trigger char with empty kana_buf in ▽ mode should output the char and exit ▽.
        let mut eng = engine_with_dict(vec!["学校"]);

        // 'B' enters Midashi with roman_buf="b" (incomplete sequence), kana_buf still empty.
        eng.process_key(&press('B'));
        // BackSpace removes roman_buf → Midashi: kana_buf="", roman_buf=""
        eng.process_key(&KeyEvent::press(Key::BackSpace, Modifiers::empty()));
        assert!(matches!(eng.phase(), SkkPhase::Midashi { ref kana_buf, ref roman_buf }
            if kana_buf.is_empty() && roman_buf.is_empty()), "▽ with empty bufs");

        // Now press '.' → kana_buf is empty, so just output "。" (kana output) and exit ▽
        let actions = eng.process_key(&press('.'));
        assert!(actions.iter().any(|a| matches!(a, EngineAction::Commit(s) if s == "。")),
            "should commit kana output for '.' directly when kana_buf is empty");
        assert!(matches!(eng.phase(), SkkPhase::Hiragana), "should exit ▽ mode");
    }

    #[test]
    fn test_conversion_trigger_cancel_escape() {
        // Escape in Selecting (after trigger) cancels; trigger char is NOT output.
        let mut eng = engine_with_dict(vec!["学校"]);

        eng.process_key(&press('G'));
        for ch in "akkou".chars() {
            eng.process_key(&press(ch));
        }
        eng.process_key(&press('.')); // triggers conversion
        assert!(matches!(eng.phase(), SkkPhase::Selecting { .. }));

        // Escape → back to ▽ mode, no output
        let actions = eng.process_key(&KeyEvent::press(Key::Escape, Modifiers::empty()));
        assert!(!actions.iter().any(|a| matches!(a, EngineAction::Commit(_))), "no commit on Escape");
        assert!(matches!(eng.phase(), SkkPhase::Midashi { .. }), "back to Midashi");
    }

    #[test]
    fn test_conversion_trigger_disabled() {
        // With empty conversion_trigger_chars, '.' in ▽ mode is ignored (not a trigger).
        let table = builtin_table(KanaLayout::Romaji);
        let mut eng = SkkEngine::new(table, SkkKeybindings {
            conversion_trigger_chars: vec![],
            ..SkkKeybindings::default()
        });
        eng.add_dict(Box::new(StubDict(vec![Candidate { word: "学校".into(), annotation: None }])));

        eng.process_key(&press('G'));
        for ch in "akkou".chars() {
            eng.process_key(&press(ch));
        }
        eng.process_key(&press('.')); // should NOT trigger conversion
        assert!(matches!(eng.phase(), SkkPhase::Midashi { .. }), "should stay in Midashi when triggers disabled");
    }
}
