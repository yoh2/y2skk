use crate::config::SkkKeybindings;
use crate::dict::entry::Candidate;
use crate::dict::traits::DictionaryProvider;
use crate::kana::table::{KanaMode, KanaTable, TransitionResult};
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
    /// Show a candidate list (vec of candidates, focused index).
    ShowCandidates(Vec<Candidate>, usize),
    /// Hide the candidate list.
    HideCandidates,
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
    /// Midashi (headword) being typed; `buf` accumulates kana
    Midashi { kana_buf: String, roman_buf: String },
    /// Okurigana being typed
    Okuri { midashi: String, okuri_prefix: char, roman_buf: String },
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
    /// New-word registration
    Register {
        midashi: String,
        okuri: Option<String>,
        buf: String,
    },
}

// ── Engine ───────────────────────────────────────────────────────────────────

pub struct SkkEngine {
    phase: SkkPhase,
    kana_state: String,
    kana_table: KanaTable,
    keybindings: SkkKeybindings,
    dict: Vec<Box<dyn DictionaryProvider>>,
}

impl SkkEngine {
    pub fn new(kana_table: KanaTable, keybindings: SkkKeybindings) -> Self {
        Self {
            phase: SkkPhase::Hiragana,
            kana_state: String::new(),
            kana_table,
            keybindings,
            dict: Vec::new(),
        }
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
        self.handle_press(event)
    }

    pub fn phase(&self) -> &SkkPhase {
        &self.phase
    }

    // ── Internal dispatch ─────────────────────────────────────────────────────

    fn handle_press(&mut self, event: &KeyEvent) -> Vec<EngineAction> {
        match &self.phase.clone() {
            SkkPhase::Ascii => self.handle_ascii(event),
            SkkPhase::Hiragana | SkkPhase::Katakana | SkkPhase::HalfWidthKatakana => {
                self.handle_kana(event)
            }
            SkkPhase::WideAscii => self.handle_wide_ascii(event),
            SkkPhase::CodeInput { prefix, buf } => {
                self.handle_code_input(event, prefix.clone(), buf.clone())
            }
            SkkPhase::Midashi { kana_buf, roman_buf } => {
                self.handle_midashi(event, kana_buf.clone(), roman_buf.clone())
            }
            SkkPhase::Okuri { midashi, okuri_prefix, roman_buf } => {
                self.handle_okuri(event, midashi.clone(), *okuri_prefix, roman_buf.clone())
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
            SkkPhase::Register { midashi, okuri, buf } => {
                self.handle_register(event, midashi.clone(), okuri.clone(), buf.clone())
            }
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

        // Escape clears any pending roman buffer
        if event.key == Key::Escape {
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit];
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
            if self.keybindings.katakana_mode.contains(&ch) {
                // q toggles between hiragana and katakana only.
                // From half-width katakana, q returns to hiragana.
                self.phase = match self.phase {
                    SkkPhase::Katakana => SkkPhase::Hiragana,
                    _ => SkkPhase::Katakana,
                };
                return vec![EngineAction::ClearPreedit];
            }
            // Uppercase letter starts midashi
            if ch.is_ascii_uppercase() {
                self.phase = SkkPhase::Midashi {
                    kana_buf: String::new(),
                    roman_buf: String::new(),
                };
                // Re-dispatch as midashi input
                return self.handle_midashi(event, String::new(), String::new());
            }
            // `\` starts code input
            if ch == '\\' {
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
            TransitionResult::NoMatch { flush, retry } => {
                self.kana_state.clear();
                let mut actions = Vec::new();
                if !flush.is_empty() {
                    actions.push(EngineAction::Commit(flush));
                }
                if let Some(retry_ch) = retry {
                    actions.extend(self.feed_kana(retry_ch, mode));
                } else {
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
        if event.key == Key::Escape {
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit];
        }

        // `\u` prefix detection (second character)
        if buf.is_empty() {
            if let Some('u') = event.printable_char() {
                self.phase = SkkPhase::CodeInput {
                    prefix: CodeInputPrefix::Unicode,
                    buf: String::new(),
                };
                return vec![self.preedit_action()];
            }
        }

        if event.key == Key::Return {
            let result = match prefix {
                CodeInputPrefix::Jis => decode_jis_code(&buf),
                CodeInputPrefix::Unicode => decode_unicode_code(&buf),
            };
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            return match result {
                Some(s) => vec![EngineAction::Commit(s), EngineAction::ClearPreedit],
                None => vec![EngineAction::ClearPreedit],
            };
        }

        if let Some(ch) = event.printable_char() {
            if ch.is_ascii_hexdigit() {
                buf.push(ch);
                self.phase = SkkPhase::CodeInput { prefix, buf };
                return vec![self.preedit_action()];
            }
        }

        vec![EngineAction::Passthrough]
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
            self.phase = SkkPhase::Hiragana;
            self.kana_state.clear();
            return vec![EngineAction::ClearPreedit];
        }

        // BackSpace
        if event.key == Key::BackSpace {
            if !roman_buf.is_empty() {
                roman_buf.pop();
            } else if !kana_buf.is_empty() {
                // Remove the last kana character
                kana_buf.pop();
            } else {
                // Empty midashi — cancel
                self.phase = SkkPhase::Hiragana;
                self.kana_state.clear();
                return vec![EngineAction::ClearPreedit];
            }
            self.phase = SkkPhase::Midashi { kana_buf, roman_buf };
            return vec![self.preedit_action()];
        }

        // Space — trigger conversion
        if event.key == Key::Space {
            // Flush any pending roman buffer as-is before lookup
            if !roman_buf.is_empty() {
                kana_buf.push_str(&roman_buf);
            }
            if kana_buf.is_empty() {
                self.phase = SkkPhase::Hiragana;
                return vec![EngineAction::ClearPreedit];
            }
            return self.start_conversion(kana_buf, None);
        }

        let Some(ch) = event.printable_char() else {
            return vec![EngineAction::Passthrough];
        };

        // Uppercase letter starts okurigana.
        // The okuri_prefix is the first consonant of the okurigana sequence:
        // - if there is a pending kana state (e.g. "w" in "KawA"), that is the prefix;
        // - otherwise the uppercase letter itself (lowercased) is the prefix.
        if ch.is_ascii_uppercase() && !kana_buf.is_empty() {
            let okuri_prefix = if let Some(first) = self.kana_state.chars().next() {
                first
            } else {
                ch.to_ascii_lowercase()
            };
            self.phase = SkkPhase::Okuri {
                midashi: kana_buf,
                okuri_prefix,
                roman_buf: String::new(),
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
                } else {
                    self.kana_state = next_state;
                }
                self.phase = SkkPhase::Midashi { kana_buf, roman_buf };
                vec![self.preedit_action()]
            }
            TransitionResult::NoMatch { flush, retry } => {
                if !flush.is_empty() {
                    kana_buf.push_str(&flush);
                }
                self.kana_state.clear();
                roman_buf.clear();
                self.phase = SkkPhase::Midashi {
                    kana_buf: kana_buf.clone(),
                    roman_buf: String::new(),
                };
                if let Some(retry_ch) = retry {
                    // Re-enter midashi handling with the retry character
                    let fake_event = KeyEvent::press(Key::Char(retry_ch), Modifiers::empty());
                    self.handle_midashi(&fake_event, kana_buf, String::new())
                } else {
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
        mut roman_buf: String,
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
        roman_buf.push(lower);

        let result = self.kana_table.transition(&self.kana_state.clone(), lower, KanaMode::Hiragana);
        match result {
            TransitionResult::Ok { output, next_state } => {
                self.kana_state = next_state;
                if !output.is_empty() {
                    // Okurigana is complete; start conversion
                    let okuri_key = self.kana_table.okuri_key(okuri_prefix).to_string();
                    self.phase = SkkPhase::Hiragana;
                    self.kana_state.clear();
                    return self.start_conversion(midashi, Some((okuri_key, output)));
                }
                self.phase = SkkPhase::Okuri { midashi, okuri_prefix, roman_buf };
                vec![self.preedit_action()]
            }
            TransitionResult::NoMatch { flush, retry } => {
                // Feed failed; treat remaining buffer as mistype
                let _ = (flush, retry);
                self.kana_state.clear();
                self.phase = SkkPhase::Okuri { midashi, okuri_prefix, roman_buf };
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
        let (okuri_key, _okuri_kana) = match &okuri {
            Some((k, v)) => (Some(k.as_str()), Some(v.as_str())),
            None => (None, None),
        };

        let candidates: Vec<Candidate> = self.dict.iter()
            .filter_map(|d| d.lookup(&midashi, okuri_key))
            .flat_map(|e| e.candidates)
            .collect();

        if candidates.is_empty() {
            // No candidates → enter registration
            self.phase = SkkPhase::Register {
                midashi,
                okuri: okuri.map(|(_, v)| v),
                buf: String::new(),
            };
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

        vec![
            EngineAction::ShowCandidates(candidates, 0),
            self.preedit_action(),
        ]
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
        // Escape / cancel key → back to midashi
        if event.key == Key::Escape
            || event.printable_char().map_or(false, |c| self.keybindings.cancel.contains(&c))
        {
            self.phase = SkkPhase::Midashi {
                kana_buf: midashi,
                roman_buf: String::new(),
            };
            self.kana_state.clear();
            return vec![EngineAction::HideCandidates, self.preedit_action()];
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

        // Space → advance to next candidate
        if event.key == Key::Space {
            index = (index + 1) % candidates.len();
            self.phase = SkkPhase::Selecting {
                midashi,
                okuri,
                okuri_key,
                candidates: candidates.clone(),
                index,
            };
            return vec![
                EngineAction::ShowCandidates(candidates, index),
                self.preedit_action(),
            ];
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

        self.phase = SkkPhase::Hiragana;
        self.kana_state.clear();
        vec![
            EngineAction::HideCandidates,
            EngineAction::Commit(commit),
            EngineAction::ClearPreedit,
        ]
    }

    fn handle_register(
        &mut self,
        event: &KeyEvent,
        midashi: String,
        okuri: Option<String>,
        mut buf: String,
    ) -> Vec<EngineAction> {
        if event.key == Key::Escape {
            self.phase = SkkPhase::Hiragana;
            return vec![EngineAction::ClearPreedit];
        }
        if event.key == Key::Return {
            // Commit the registered word
            let commit = format!("{}{}", buf, okuri.as_deref().unwrap_or(""));
            // TODO: persist to user dict
            self.phase = SkkPhase::Hiragana;
            return vec![EngineAction::Commit(commit), EngineAction::ClearPreedit];
        }
        if event.key == Key::BackSpace {
            buf.pop();
            self.phase = SkkPhase::Register { midashi, okuri, buf };
            return vec![self.preedit_action()];
        }
        if let Some(ch) = event.printable_char() {
            buf.push(ch);
            self.phase = SkkPhase::Register { midashi, okuri, buf };
            return vec![self.preedit_action()];
        }
        vec![EngineAction::Passthrough]
    }

    // ── Preedit helpers ───────────────────────────────────────────────────────

    fn preedit_action(&self) -> EngineAction {
        EngineAction::UpdatePreedit(self.build_preedit())
    }

    fn build_preedit(&self) -> Preedit {
        let text = match &self.phase {
            SkkPhase::Hiragana | SkkPhase::Katakana | SkkPhase::HalfWidthKatakana => {
                self.kana_state.clone()
            }
            SkkPhase::WideAscii | SkkPhase::Ascii => String::new(),
            SkkPhase::CodeInput { prefix, buf } => {
                let prefix_str = match prefix {
                    CodeInputPrefix::Jis => "\\",
                    CodeInputPrefix::Unicode => "\\u",
                };
                format!("{prefix_str}{buf}")
            }
            SkkPhase::Midashi { kana_buf, roman_buf } => {
                format!("▽{kana_buf}{roman_buf}")
            }
            SkkPhase::Okuri { midashi, okuri_prefix, roman_buf } => {
                format!("▽{midashi}*{okuri_prefix}{roman_buf}")
            }
            SkkPhase::Selecting { midashi: _, okuri, okuri_key: _, candidates, index } => {
                let cand = &candidates[*index];
                let ok = okuri.as_deref().unwrap_or("");
                match &cand.annotation {
                    Some(ann) => format!("▼{}{};{}", cand.word, ok, ann),
                    None => format!("▼{}{}", cand.word, ok),
                }
            }
            SkkPhase::Register { midashi, okuri, buf } => {
                let ok = okuri.as_deref().unwrap_or("");
                format!("({midashi}{ok}): {buf}")
            }
        };
        let cursor = text.len();
        Preedit { text, cursor }
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
fn decode_jis_code(hex: &str) -> Option<String> {
    if hex.len() != 4 {
        return None;
    }
    let code = u32::from_str_radix(hex, 16).ok()?;
    // JIS X 0208: row-cell (ku-ten) encoding; rough mapping via EUC-JP
    // For now, treat as a Unicode code point directly (full implementation requires EUC-JP table)
    char::from_u32(code).map(|c| c.to_string())
}

/// Decodes a Unicode code point (1–6 hex digits) to a string.
fn decode_unicode_code(hex: &str) -> Option<String> {
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
}
