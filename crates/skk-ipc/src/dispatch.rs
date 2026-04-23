use crate::{
    IpcAction, NO_GHOST,
    ACTION_CLEAR_PREEDIT, ACTION_COMMIT, ACTION_HIDE_CANDIDATES, ACTION_PASSTHROUGH,
    ACTION_SHOW_CANDIDATES, ACTION_UPDATE_PREEDIT, ACTION_UPDATE_STATUS,
};

/// Receiver of IME actions produced by dispatching a `Vec<IpcAction>`.
///
/// Each platform adapter implements this trait to translate engine output into
/// protocol-specific operations (XIM commit, GTK preedit signal, etc.).
pub trait ActionSink {
    fn commit(&mut self, text: &str);
    /// `ghost_start`: `None` = no ghost preview, `Some(byte_offset)` = ghost begins here.
    fn update_preedit(&mut self, text: &str, cursor: u32, ghost_start: Option<u32>);
    fn clear_preedit(&mut self);
    fn show_candidates(&mut self, candidates: &[String], focused: u32, sel_keys: &str);
    fn hide_candidates(&mut self);
    /// `timeout_ms`: hint for how long to show the status indicator (0 = persistent).
    fn update_status(&mut self, indicator: &str, timeout_ms: u32);
}

/// Outcome of dispatching a batch of `IpcAction`s.
pub struct DispatchResult {
    /// True if any action consumed the key (i.e. the adapter should not pass it through).
    pub consumed: bool,
    /// True if `ACTION_PASSTHROUGH` was present, forcing the key through even when consumed.
    pub force_passthrough: bool,
}

/// Dispatch a slice of `IpcAction`s through `sink` and return the consumed/passthrough flags.
///
/// All adapters should use this function instead of hand-rolling their own match blocks.
/// The consumed-flag semantics:
///   - PASSTHROUGH sets `force_passthrough`; the key is passed to the app regardless.
///   - All other handled actions (COMMIT, UPDATE_PREEDIT, …, UPDATE_STATUS) set `consumed`.
///   - The caller should forward the key when `!consumed || force_passthrough`.
pub fn dispatch<S: ActionSink>(actions: &[IpcAction], sink: &mut S) -> DispatchResult {
    let mut consumed = false;
    let mut force_passthrough = false;

    for action in actions {
        match action.kind {
            k if k == ACTION_PASSTHROUGH => {
                // Passthrough can accompany commit actions (e.g. Return confirms a candidate
                // AND forwards Enter to the application).
                force_passthrough = true;
            }
            k if k == ACTION_COMMIT => {
                consumed = true;
                sink.commit(&action.text);
            }
            k if k == ACTION_UPDATE_PREEDIT => {
                consumed = true;
                let ghost = (action.ghost_start != NO_GHOST).then_some(action.ghost_start);
                sink.update_preedit(&action.text, action.cursor, ghost);
            }
            k if k == ACTION_CLEAR_PREEDIT => {
                consumed = true;
                sink.clear_preedit();
            }
            k if k == ACTION_SHOW_CANDIDATES => {
                consumed = true;
                // action.text carries the selection-key string for the candidate list.
                sink.show_candidates(&action.candidates, action.focused, &action.text);
            }
            k if k == ACTION_HIDE_CANDIDATES => {
                consumed = true;
                sink.hide_candidates();
            }
            k if k == ACTION_UPDATE_STATUS => {
                // Consumed: mode-switch keys (l, q, Shift+Space, …) must not reach the app.
                // action.cursor carries the display timeout in milliseconds.
                consumed = true;
                sink.update_status(&action.text, action.cursor);
            }
            _ => {}
        }
    }

    DispatchResult { consumed, force_passthrough }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IpcAction;

    #[derive(Default)]
    struct MockSink {
        committed: Vec<String>,
        preedit: Option<(String, u32, Option<u32>)>,
        preedit_cleared: bool,
        candidates_shown: Option<(Vec<String>, u32, String)>,
        candidates_hidden: bool,
        status: Option<(String, u32)>,
    }

    impl ActionSink for MockSink {
        fn commit(&mut self, text: &str) {
            self.committed.push(text.to_string());
        }
        fn update_preedit(&mut self, text: &str, cursor: u32, ghost_start: Option<u32>) {
            self.preedit = Some((text.to_string(), cursor, ghost_start));
        }
        fn clear_preedit(&mut self) {
            self.preedit_cleared = true;
        }
        fn show_candidates(&mut self, candidates: &[String], focused: u32, sel_keys: &str) {
            self.candidates_shown = Some((candidates.to_vec(), focused, sel_keys.to_string()));
        }
        fn hide_candidates(&mut self) {
            self.candidates_hidden = true;
        }
        fn update_status(&mut self, indicator: &str, timeout_ms: u32) {
            self.status = Some((indicator.to_string(), timeout_ms));
        }
    }

    #[test]
    fn passthrough_not_consumed() {
        let mut sink = MockSink::default();
        let r = dispatch(&[IpcAction::passthrough()], &mut sink);
        assert!(!r.consumed);
        assert!(r.force_passthrough);
    }

    #[test]
    fn commit_is_consumed() {
        let mut sink = MockSink::default();
        let r = dispatch(&[IpcAction::commit("あ")], &mut sink);
        assert!(r.consumed);
        assert!(!r.force_passthrough);
        assert_eq!(sink.committed, vec!["あ"]);
    }

    #[test]
    fn commit_with_passthrough() {
        let mut sink = MockSink::default();
        let r = dispatch(&[IpcAction::commit("あ"), IpcAction::passthrough()], &mut sink);
        assert!(r.consumed);
        assert!(r.force_passthrough);
        assert_eq!(sink.committed, vec!["あ"]);
    }

    #[test]
    fn update_status_is_consumed() {
        let mut sink = MockSink::default();
        let r = dispatch(&[IpcAction::update_status("あ")], &mut sink);
        assert!(r.consumed);
        assert!(!r.force_passthrough);
        assert_eq!(sink.status, Some(("あ".to_string(), 0)));
    }

    #[test]
    fn preedit_no_ghost_normalised_to_none() {
        let mut sink = MockSink::default();
        dispatch(&[IpcAction::update_preedit("か", 3, NO_GHOST)], &mut sink);
        assert_eq!(sink.preedit, Some(("か".to_string(), 3, None)));
    }

    #[test]
    fn preedit_ghost_preserved() {
        let mut sink = MockSink::default();
        dispatch(&[IpcAction::update_preedit("かいかわらず", 3, 3)], &mut sink);
        assert_eq!(sink.preedit, Some(("かいかわらず".to_string(), 3, Some(3))));
    }

    #[test]
    fn empty_actions_not_consumed() {
        let mut sink = MockSink::default();
        let r = dispatch(&[], &mut sink);
        assert!(!r.consumed);
        assert!(!r.force_passthrough);
    }
}
