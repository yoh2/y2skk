use std::collections::HashMap;

/// A single entry in a kana conversion table.
/// Represents the transition rule `(from, input) → (to, output)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KanaTransition {
    /// Current intermediate state ("" = start state)
    pub from: String,
    /// Input character
    pub input: char,
    /// Next state after transition ("" = return to start state)
    pub to: String,
    /// Output kana string ("" = no output yet)
    pub output: String,
}

/// Kana conversion table.
#[derive(Debug, Clone)]
pub struct KanaTable {
    /// Transition rules from `[trans]` (hiragana output)
    transitions: Vec<KanaTransition>,
    /// Katakana-mode overrides from `[trans.katakana]`
    kata_overrides: Vec<KanaTransition>,
    /// Okurigana dictionary key aliases from `[okuri_alias]` (input_char → alias_char)
    okuri_aliases: HashMap<char, char>,
}

/// Kana conversion mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KanaMode {
    Hiragana,
    Katakana,
    HalfWidth,
}

/// Built-in kana layout names
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KanaLayout {
    Romaji,
    AzikUs,
    AzikJp,
    DvorakJpUs,
    DvorakJpJp,
}

/// Result of a state machine transition
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionResult {
    /// Transition succeeded. `output` holds the kana string (empty if not yet finalized),
    /// `next_state` holds the new state.
    Ok { output: String, next_state: String },
    /// No rule matched the current state and input (mistype).
    /// `flush` contains the intermediate buffer to discard and output as-is (if any).
    /// `retry` contains the input character to re-process from the start state (if any).
    NoMatch { flush: String, retry: Option<char> },
}

impl KanaTable {
    pub fn new(
        transitions: Vec<KanaTransition>,
        kata_overrides: Vec<KanaTransition>,
        okuri_aliases: HashMap<char, char>,
    ) -> Self {
        Self { transitions, kata_overrides, okuri_aliases }
    }

    /// Overlays a user-defined table on top of this table.
    /// Used to partially override a built-in table with user-defined rules.
    pub fn overlay(mut self, user: KanaTable) -> Self {
        for ut in &user.transitions {
            self.transitions.retain(|t| !(t.from == ut.from && t.input == ut.input));
        }
        self.transitions.extend(user.transitions);

        for ut in &user.kata_overrides {
            self.kata_overrides.retain(|t| !(t.from == ut.from && t.input == ut.input));
        }
        self.kata_overrides.extend(user.kata_overrides);

        for (k, v) in user.okuri_aliases {
            self.okuri_aliases.insert(k, v);
        }
        self
    }

    /// Returns the transition result for the given state and input character.
    /// Wildcard rules (where `from` is `"*"`) are used as a fallback when no exact rule matches.
    pub fn transition(&self, state: &str, input: char, mode: KanaMode) -> TransitionResult {
        let transitions = self.effective_transitions(mode);

        // Exact match
        if let Some(t) = transitions.iter().find(|t| t.from == state && t.input == input) {
            let output = self.convert_output(&t.output, mode);
            return TransitionResult::Ok { output, next_state: t.to.clone() };
        }

        // Wildcard match: `\*` in the table file is stored as input == '*' and matches any char
        if let Some(t) = transitions.iter().find(|t| t.from == state && t.input == '*') {
            let output = self.convert_output(&t.output, mode);
            return TransitionResult::Ok { output, next_state: t.to.clone() };
        }

        // No match from start state: just ignore the input
        if state.is_empty() {
            return TransitionResult::NoMatch { flush: String::new(), retry: None };
        }

        // No match from an intermediate state: flush the buffer and retry the input
        TransitionResult::NoMatch { flush: state.to_string(), retry: Some(input) }
    }

    /// Resolves the okurigana dictionary key for the given input character.
    /// Returns the alias if registered in `[okuri_alias]`, otherwise returns the character unchanged.
    pub fn okuri_key(&self, input: char) -> char {
        self.okuri_aliases.get(&input).copied().unwrap_or(input)
    }

    /// Returns the effective transition list for the given mode.
    fn effective_transitions(&self, mode: KanaMode) -> Vec<&KanaTransition> {
        match mode {
            KanaMode::Hiragana => self.transitions.iter().collect(),
            KanaMode::Katakana | KanaMode::HalfWidth => {
                // kata_overrides take priority; fall back to transitions for the rest
                let mut result: Vec<&KanaTransition> = Vec::new();
                for t in &self.transitions {
                    if !self.kata_overrides.iter().any(|o| o.from == t.from && o.input == t.input) {
                        result.push(t);
                    }
                }
                result.extend(self.kata_overrides.iter());
                result
            }
        }
    }

    /// Converts a hiragana output string to the target mode.
    fn convert_output(&self, output: &str, mode: KanaMode) -> String {
        match mode {
            KanaMode::Hiragana => output.to_string(),
            KanaMode::Katakana => hiragana_to_katakana(output),
            KanaMode::HalfWidth => hiragana_to_halfwidth(output),
        }
    }
}

/// Converts hiragana to katakana using Unicode offsets (U+3041–U+3096 → U+30A1–U+30F6).
fn hiragana_to_katakana(s: &str) -> String {
    s.chars().map(|c| {
        if ('\u{3041}'..='\u{3096}').contains(&c) {
            char::from_u32(c as u32 + 0x60).unwrap_or(c)
        } else {
            c
        }
    }).collect()
}

/// Converts hiragana to half-width katakana.
fn hiragana_to_halfwidth(s: &str) -> String {
    s.chars().flat_map(to_halfwidth_katakana).collect()
}

/// Converts a single character to half-width katakana.
/// Characters with dakuten/handakuten are decomposed into two half-width characters.
fn to_halfwidth_katakana(c: char) -> Vec<char> {
    match hiragana_to_katakana(&c.to_string()).chars().next().unwrap_or(c) {
        'ァ' => vec!['ｧ'],
        'ア' => vec!['ｱ'],
        'ィ' => vec!['ｨ'],
        'イ' => vec!['ｲ'],
        'ゥ' => vec!['ｩ'],
        'ウ' => vec!['ｳ'],
        'ェ' => vec!['ｪ'],
        'エ' => vec!['ｴ'],
        'ォ' => vec!['ｫ'],
        'オ' => vec!['ｵ'],
        'カ' => vec!['ｶ'],
        'ガ' => vec!['ｶ', 'ﾞ'],
        'キ' => vec!['ｷ'],
        'ギ' => vec!['ｷ', 'ﾞ'],
        'ク' => vec!['ｸ'],
        'グ' => vec!['ｸ', 'ﾞ'],
        'ケ' => vec!['ｹ'],
        'ゲ' => vec!['ｹ', 'ﾞ'],
        'コ' => vec!['ｺ'],
        'ゴ' => vec!['ｺ', 'ﾞ'],
        'サ' => vec!['ｻ'],
        'ザ' => vec!['ｻ', 'ﾞ'],
        'シ' => vec!['ｼ'],
        'ジ' => vec!['ｼ', 'ﾞ'],
        'ス' => vec!['ｽ'],
        'ズ' => vec!['ｽ', 'ﾞ'],
        'セ' => vec!['ｾ'],
        'ゼ' => vec!['ｾ', 'ﾞ'],
        'ソ' => vec!['ｿ'],
        'ゾ' => vec!['ｿ', 'ﾞ'],
        'タ' => vec!['ﾀ'],
        'ダ' => vec!['ﾀ', 'ﾞ'],
        'チ' => vec!['ﾁ'],
        'ヂ' => vec!['ﾁ', 'ﾞ'],
        'ッ' => vec!['ｯ'],
        'ツ' => vec!['ﾂ'],
        'ヅ' => vec!['ﾂ', 'ﾞ'],
        'テ' => vec!['ﾃ'],
        'デ' => vec!['ﾃ', 'ﾞ'],
        'ト' => vec!['ﾄ'],
        'ド' => vec!['ﾄ', 'ﾞ'],
        'ナ' => vec!['ﾅ'],
        'ニ' => vec!['ﾆ'],
        'ヌ' => vec!['ﾇ'],
        'ネ' => vec!['ﾈ'],
        'ノ' => vec!['ﾉ'],
        'ハ' => vec!['ﾊ'],
        'バ' => vec!['ﾊ', 'ﾞ'],
        'パ' => vec!['ﾊ', 'ﾟ'],
        'ヒ' => vec!['ﾋ'],
        'ビ' => vec!['ﾋ', 'ﾞ'],
        'ピ' => vec!['ﾋ', 'ﾟ'],
        'フ' => vec!['ﾌ'],
        'ブ' => vec!['ﾌ', 'ﾞ'],
        'プ' => vec!['ﾌ', 'ﾟ'],
        'ヘ' => vec!['ﾍ'],
        'ベ' => vec!['ﾍ', 'ﾞ'],
        'ペ' => vec!['ﾍ', 'ﾟ'],
        'ホ' => vec!['ﾎ'],
        'ボ' => vec!['ﾎ', 'ﾞ'],
        'ポ' => vec!['ﾎ', 'ﾟ'],
        'マ' => vec!['ﾏ'],
        'ミ' => vec!['ﾐ'],
        'ム' => vec!['ﾑ'],
        'メ' => vec!['ﾒ'],
        'モ' => vec!['ﾓ'],
        'ャ' => vec!['ｬ'],
        'ヤ' => vec!['ﾔ'],
        'ュ' => vec!['ｭ'],
        'ユ' => vec!['ﾕ'],
        'ョ' => vec!['ｮ'],
        'ヨ' => vec!['ﾖ'],
        'ラ' => vec!['ﾗ'],
        'リ' => vec!['ﾘ'],
        'ル' => vec!['ﾙ'],
        'レ' => vec!['ﾚ'],
        'ロ' => vec!['ﾛ'],
        'ワ' => vec!['ﾜ'],
        'ヲ' => vec!['ｦ'],
        'ン' => vec!['ﾝ'],
        'ヴ' => vec!['ｳ', 'ﾞ'],
        'ヵ' => vec!['ｶ'],
        'ヶ' => vec!['ｹ'],
        'ー' => vec!['ｰ'],
        '。' => vec!['｡'],
        '「' => vec!['｢'],
        '」' => vec!['｣'],
        '、' => vec!['､'],
        '・' => vec!['･'],
        other => vec![other],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_table(entries: &[(&str, char, &str, &str)]) -> KanaTable {
        let transitions = entries.iter().map(|(from, input, to, output)| {
            KanaTransition {
                from: from.to_string(),
                input: *input,
                to: to.to_string(),
                output: output.to_string(),
            }
        }).collect();
        KanaTable::new(transitions, vec![], HashMap::new())
    }

    #[test]
    fn test_basic_transition() {
        let table = make_table(&[
            ("", 'k', "k", ""),
            ("k", 'a', "", "か"),
            ("k", 'i', "", "き"),
        ]);

        let r = table.transition("", 'k', KanaMode::Hiragana);
        assert_eq!(r, TransitionResult::Ok { output: String::new(), next_state: "k".into() });

        let r = table.transition("k", 'a', KanaMode::Hiragana);
        assert_eq!(r, TransitionResult::Ok { output: "か".into(), next_state: String::new() });
    }

    #[test]
    fn test_katakana_auto_derive() {
        let table = make_table(&[
            ("", 'k', "k", ""),
            ("k", 'a', "", "か"),
        ]);
        let r = table.transition("k", 'a', KanaMode::Katakana);
        assert_eq!(r, TransitionResult::Ok { output: "カ".into(), next_state: String::new() });
    }

    #[test]
    fn test_no_match_intermediate() {
        let table = make_table(&[
            ("", 'k', "k", ""),
            ("k", 'a', "", "か"),
        ]);
        // 'b' is not reachable from state "k" → flush="k", retry=Some('b')
        let r = table.transition("k", 'b', KanaMode::Hiragana);
        assert_eq!(r, TransitionResult::NoMatch { flush: "k".into(), retry: Some('b') });
    }

    #[test]
    fn test_hiragana_to_katakana() {
        assert_eq!(hiragana_to_katakana("かきくけこ"), "カキクケコ");
        assert_eq!(hiragana_to_katakana("っ"), "ッ");
    }
}
