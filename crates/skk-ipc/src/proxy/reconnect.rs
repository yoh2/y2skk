//! Reconnecting wrapper around [`super::blocking::DaemonProxy`].
//!
//! Adapters (GTK3, Qt6, XIM) use this client instead of calling
//! [`super::blocking::DaemonProxy`] directly.  It adds:
//!
//! * **Local session handles** — the caller gets an opaque `u32` handle.
//!   The actual daemon `SessionId` is held internally and replaced
//!   transparently when the daemon restarts.
//!
//! * **Automatic session re-creation** — when the daemon responds with
//!   [`ACTION_SESSION_INVALID`] (unknown session, e.g. after a restart),
//!   the client re-creates the session and retries the key once.
//!
//! * **Fail-open with background recovery** — on a D-Bus failure the
//!   client immediately returns [`ProcessKeyError::Unavailable`] (no
//!   blocking wait) and notifies a background monitor thread.  The monitor
//!   probes the daemon with exponential backoff; once the daemon is back,
//!   it clears the failure state so subsequent keystrokes resume normally.
//!   This means UI-thread typing is blocked only for the *first* failed
//!   call (one D-Bus timeout), never again until the daemon recovers.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::blocking::DaemonProxy;
use crate::{IpcAction, SessionId, ACTION_SESSION_INVALID};

/// Initial retry delay after a failure.
const INITIAL_COOLDOWN: Duration = Duration::from_millis(2000);
/// Maximum delay between background recovery probes.
const MAX_COOLDOWN: Duration = Duration::from_millis(30_000);

/// Opaque per-IC handle handed out to callers.
pub type LocalHandle = u32;

/// State associated with one handle (one IM context).
struct SessionEntry {
    app_id: String,
    /// Daemon-side session ID; 0 means "not yet created / needs creation".
    daemon_session_id: SessionId,
}

/// Failure tracking state shared between the UI thread and the monitor thread.
struct FailureState {
    last_failure: Option<Instant>,
    consecutive_failures: u32,
}

/// State shared between the UI thread and the background monitor thread.
struct Shared {
    proxy: Mutex<DaemonProxy>,
    sessions: Mutex<HashMap<LocalHandle, SessionEntry>>,
    failure: Mutex<FailureState>,
    /// Condvar used to wake the monitor thread when a failure is detected.
    wake: Condvar,
    /// Set to true to signal the monitor thread to exit.
    shutdown: AtomicBool,
}

impl Shared {
    fn in_cooldown(&self) -> bool {
        let f = self.failure.lock().unwrap();
        match f.last_failure {
            Some(t) => t.elapsed() < current_cooldown(f.consecutive_failures),
            None => false,
        }
    }

    /// Records a failure, invalidates all daemon session IDs, and wakes
    /// the monitor thread.
    fn mark_failed(&self) {
        {
            let mut f = self.failure.lock().unwrap();
            f.consecutive_failures = f.consecutive_failures.saturating_add(1);
            f.last_failure = Some(Instant::now());
        }
        // Invalidate all known sessions so they are lazily re-created after
        // recovery instead of trying to use stale IDs.
        let mut s = self.sessions.lock().unwrap();
        for e in s.values_mut() {
            e.daemon_session_id = 0;
        }
        drop(s);
        self.wake.notify_all();
    }

    /// Clears the failure state after a successful probe or D-Bus call.
    fn mark_recovered(&self) {
        let mut f = self.failure.lock().unwrap();
        f.consecutive_failures = 0;
        f.last_failure = None;
    }
}

/// Error returned by [`ReconnectingClient::process_key`].
#[derive(Debug)]
pub enum ProcessKeyError {
    /// Daemon is unreachable or all reconnect attempts failed.
    /// The caller should treat the key as a passthrough.
    Unavailable,
    /// The provided handle was never registered.  Indicates a caller bug.
    UnknownHandle,
}

/// Reconnecting, fail-open D-Bus client for `org.y2skk.Daemon`.
///
/// Thread-safe via `Mutex` guards; GTK3 and Qt6 call this from the UI
/// thread only, and the XIM server's event loop is also single-threaded,
/// so lock contention is effectively zero in practice.
pub struct ReconnectingClient {
    shared: Arc<Shared>,
    next_handle: AtomicU32,
    // Kept alive for process lifetime; the thread name helps with debugging.
    _monitor: thread::JoinHandle<()>,
}

/// Returns the cooldown duration for the given consecutive failure count.
fn current_cooldown(failures: u32) -> Duration {
    // 2s, 4s, 8s, 16s, 30s, 30s, …
    let shift = failures.saturating_sub(1).min(16);
    let d = INITIAL_COOLDOWN.saturating_mul(1u32 << shift);
    std::cmp::min(d, MAX_COOLDOWN)
}

/// Background loop that probes the daemon and clears the failure state once
/// the daemon is reachable again.
fn monitor_loop(shared: Arc<Shared>) {
    loop {
        // Sleep until a failure is reported.
        {
            let guard = shared.failure.lock().unwrap();
            let _g = shared
                .wake
                .wait_while(guard, |f| {
                    !shared.shutdown.load(Ordering::Relaxed) && f.last_failure.is_none()
                })
                .unwrap();
        }
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }

        // Probe with exponential backoff until the daemon responds.
        loop {
            let wait = {
                let f = shared.failure.lock().unwrap();
                current_cooldown(f.consecutive_failures)
            };
            thread::sleep(wait);
            if shared.shutdown.load(Ordering::Relaxed) {
                return;
            }

            let probe = {
                let proxy = shared.proxy.lock().unwrap();
                proxy.create_session("y2skk-probe")
            };
            match probe {
                Ok(id) => {
                    let _ = shared.proxy.lock().unwrap().destroy_session(id);
                    shared.mark_recovered();
                    break; // Go back to waiting for the next failure.
                }
                Err(e) => {
                    eprintln!("y2skk: recovery probe failed: {e}");
                    let mut f = shared.failure.lock().unwrap();
                    f.consecutive_failures = f.consecutive_failures.saturating_add(1);
                    f.last_failure = Some(Instant::now());
                }
            }
        }
    }
}

impl ReconnectingClient {
    /// Connects to the session D-Bus, spawns the monitor thread, and
    /// returns a ready client.
    pub fn new() -> zbus::Result<Self> {
        let proxy = DaemonProxy::connect()?;
        let shared = Arc::new(Shared {
            proxy: Mutex::new(proxy),
            sessions: Mutex::new(HashMap::new()),
            failure: Mutex::new(FailureState {
                last_failure: None,
                consecutive_failures: 0,
            }),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let monitor = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("y2skk-reconnect-monitor".into())
                .spawn(move || monitor_loop(shared))
                .expect("failed to spawn reconnect monitor thread")
        };
        Ok(Self {
            shared,
            next_handle: AtomicU32::new(1),
            _monitor: monitor,
        })
    }

    /// Allocates a new local handle for an IM context.
    ///
    /// If the daemon is currently unreachable (cooldown active), the handle
    /// is returned immediately with a deferred session ID of 0; the real
    /// daemon session will be created on the first successful
    /// [`Self::process_key`] call.
    pub fn create_handle(&self, app_id: &str) -> LocalHandle {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        let daemon_session_id = if self.shared.in_cooldown() {
            0
        } else {
            match self.try_create_daemon_session(app_id) {
                Some(id) => id,
                None => {
                    self.shared.mark_failed();
                    0
                }
            }
        };
        self.shared.sessions.lock().unwrap().insert(
            handle,
            SessionEntry {
                app_id: app_id.to_string(),
                daemon_session_id,
            },
        );
        handle
    }

    /// Destroys the daemon session associated with `handle`.
    ///
    /// During cooldown the D-Bus call is skipped to avoid blocking; any
    /// stale server-side session will be cleaned up naturally via
    /// `ACTION_SESSION_INVALID` handling after recovery.
    pub fn destroy_handle(&self, handle: LocalHandle) {
        let entry = self.shared.sessions.lock().unwrap().remove(&handle);
        if let Some(e) = entry {
            if e.daemon_session_id != 0 && !self.shared.in_cooldown() {
                let proxy = self.shared.proxy.lock().unwrap();
                if let Err(err) = proxy.destroy_session(e.daemon_session_id) {
                    eprintln!("y2skk: destroy_session D-Bus error: {err}");
                }
            }
        }
    }

    /// Sends a key event to the daemon and returns the resulting actions.
    ///
    /// Returns [`ProcessKeyError::Unavailable`] immediately when a cooldown
    /// is active (daemon unreachable); the background monitor thread will
    /// probe and clear the cooldown independently.
    ///
    /// On [`ACTION_SESSION_INVALID`] (daemon restarted but session stale)
    /// the session is recreated inline and the key is retried once.
    ///
    /// On any other D-Bus error the failure is recorded and the method
    /// returns [`ProcessKeyError::Unavailable`] immediately — no retry,
    /// no additional blocking.
    pub fn process_key(
        &self,
        handle: LocalHandle,
        key_sym: u32,
        modifiers: u32,
        is_press: bool,
    ) -> Result<Vec<IpcAction>, ProcessKeyError> {
        if self.shared.in_cooldown() {
            return Err(ProcessKeyError::Unavailable);
        }

        let daemon_id = self.ensure_daemon_session(handle)?;

        match self.call_process_key(daemon_id, key_sym, modifiers, is_press) {
            Ok(actions) if actions.iter().any(|a| a.kind == ACTION_SESSION_INVALID) => {
                // Daemon is alive but the session is stale (e.g. daemon restarted).
                // Recreate the session and retry exactly once.
                match self.recreate_daemon_session(handle) {
                    Some(new_id) => {
                        match self.call_process_key(new_id, key_sym, modifiers, is_press) {
                            Ok(a) => {
                                self.shared.mark_recovered();
                                Ok(a)
                            }
                            Err(e) => {
                                eprintln!("y2skk: process_key retry failed: {e}");
                                self.shared.mark_failed();
                                Err(ProcessKeyError::Unavailable)
                            }
                        }
                    }
                    None => {
                        self.shared.mark_failed();
                        Err(ProcessKeyError::Unavailable)
                    }
                }
            }
            Ok(actions) => {
                self.shared.mark_recovered();
                Ok(actions)
            }
            Err(e) => {
                // Fail-fast: do not attempt to recreate the session here.
                // The monitor thread will probe the daemon asynchronously.
                eprintln!("y2skk: process_key D-Bus error: {e}");
                self.shared.mark_failed();
                Err(ProcessKeyError::Unavailable)
            }
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Returns the daemon session ID for `handle`, creating one if needed.
    fn ensure_daemon_session(
        &self,
        handle: LocalHandle,
    ) -> Result<SessionId, ProcessKeyError> {
        let mut sessions = self.shared.sessions.lock().unwrap();
        let entry = sessions
            .get_mut(&handle)
            .ok_or(ProcessKeyError::UnknownHandle)?;
        if entry.daemon_session_id != 0 {
            return Ok(entry.daemon_session_id);
        }
        // Session not yet created (deferred at create_handle time); try now.
        let app_id = entry.app_id.clone();
        drop(sessions);
        match self.try_create_daemon_session(&app_id) {
            Some(id) => {
                self.shared
                    .sessions
                    .lock()
                    .unwrap()
                    .get_mut(&handle)
                    .unwrap()
                    .daemon_session_id = id;
                Ok(id)
            }
            None => {
                self.shared.mark_failed();
                Err(ProcessKeyError::Unavailable)
            }
        }
    }

    /// Creates a fresh daemon session for `handle`; returns the new ID or
    /// `None` on error.
    fn recreate_daemon_session(&self, handle: LocalHandle) -> Option<SessionId> {
        let app_id = {
            let sessions = self.shared.sessions.lock().unwrap();
            sessions.get(&handle)?.app_id.clone()
        };
        let new_id = self.try_create_daemon_session(&app_id)?;
        if let Some(entry) = self.shared.sessions.lock().unwrap().get_mut(&handle) {
            entry.daemon_session_id = new_id;
        }
        Some(new_id)
    }

    fn try_create_daemon_session(&self, app_id: &str) -> Option<SessionId> {
        let proxy = self.shared.proxy.lock().unwrap();
        match proxy.create_session(app_id) {
            Ok(id) => Some(id),
            Err(e) => {
                eprintln!("y2skk: create_session D-Bus error: {e}");
                None
            }
        }
    }

    fn call_process_key(
        &self,
        session_id: SessionId,
        key_sym: u32,
        modifiers: u32,
        is_press: bool,
    ) -> zbus::Result<Vec<IpcAction>> {
        let proxy = self.shared.proxy.lock().unwrap();
        proxy.process_key(session_id, key_sym, modifiers, is_press)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ACTION_PASSTHROUGH;

    /// Minimal mock transport that the tests drive without a real daemon.
    struct MockTransport {
        /// Responses to return for each successive `process_key` call.
        responses: Mutex<std::collections::VecDeque<zbus::Result<Vec<IpcAction>>>>,
        /// IDs to hand out for each successive `create_session` call.
        session_ids: Mutex<std::collections::VecDeque<zbus::Result<SessionId>>>,
    }

    impl MockTransport {
        fn new(
            session_ids: Vec<zbus::Result<SessionId>>,
            responses: Vec<zbus::Result<Vec<IpcAction>>>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                session_ids: Mutex::new(session_ids.into()),
            }
        }
        fn create_session(&self) -> zbus::Result<SessionId> {
            self.session_ids
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected create_session call")
        }
        fn process_key(&self) -> zbus::Result<Vec<IpcAction>> {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected process_key call")
        }
    }

    /// A `ReconnectingClient`-like struct that uses `MockTransport` instead of
    /// a real D-Bus connection.  Duplicates a subset of `ReconnectingClient`
    /// logic to keep the production code free of trait indirection.
    struct MockClient {
        transport: MockTransport,
        sessions: Mutex<HashMap<LocalHandle, SessionEntry>>,
        next_handle: AtomicU32,
        failure: Mutex<FailureState>,
    }

    impl MockClient {
        fn new(transport: MockTransport) -> Self {
            Self {
                transport,
                sessions: Mutex::new(HashMap::new()),
                next_handle: AtomicU32::new(1),
                failure: Mutex::new(FailureState {
                    last_failure: None,
                    consecutive_failures: 0,
                }),
            }
        }

        fn create_handle(&self, app_id: &str) -> LocalHandle {
            let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
            let daemon_session_id = if self.in_cooldown() {
                0
            } else {
                match self.transport.create_session() {
                    Ok(id) => id,
                    Err(_) => { self.mark_failed(); 0 }
                }
            };
            self.sessions.lock().unwrap().insert(
                handle,
                SessionEntry { app_id: app_id.to_string(), daemon_session_id },
            );
            handle
        }

        fn in_cooldown(&self) -> bool {
            let f = self.failure.lock().unwrap();
            match f.last_failure {
                Some(t) => t.elapsed() < current_cooldown(f.consecutive_failures),
                None => false,
            }
        }

        fn mark_failed(&self) {
            let mut f = self.failure.lock().unwrap();
            f.consecutive_failures = f.consecutive_failures.saturating_add(1);
            f.last_failure = Some(Instant::now());
            // Also invalidate sessions.
            drop(f);
            let mut s = self.sessions.lock().unwrap();
            for e in s.values_mut() {
                e.daemon_session_id = 0;
            }
        }

        fn mark_recovered(&self) {
            let mut f = self.failure.lock().unwrap();
            f.consecutive_failures = 0;
            f.last_failure = None;
        }

        fn consecutive_failures(&self) -> u32 {
            self.failure.lock().unwrap().consecutive_failures
        }

        fn ensure_daemon_session(&self, handle: LocalHandle) -> Result<SessionId, ProcessKeyError> {
            let mut sessions = self.sessions.lock().unwrap();
            let entry = sessions.get_mut(&handle).ok_or(ProcessKeyError::UnknownHandle)?;
            if entry.daemon_session_id != 0 {
                return Ok(entry.daemon_session_id);
            }
            let _app_id = entry.app_id.clone();
            drop(sessions);
            match self.transport.create_session() {
                Ok(id) => {
                    self.sessions.lock().unwrap().get_mut(&handle).unwrap().daemon_session_id = id;
                    Ok(id)
                }
                Err(_) => {
                    self.mark_failed();
                    Err(ProcessKeyError::Unavailable)
                }
            }
        }

        fn recreate_daemon_session(&self, handle: LocalHandle) -> Option<SessionId> {
            let _app_id = {
                let sessions = self.sessions.lock().unwrap();
                sessions.get(&handle)?.app_id.clone()
            };
            let new_id = self.transport.create_session().ok()?;
            if let Some(entry) = self.sessions.lock().unwrap().get_mut(&handle) {
                entry.daemon_session_id = new_id;
            }
            Some(new_id)
        }

        fn process_key(&self, handle: LocalHandle) -> Result<Vec<IpcAction>, ProcessKeyError> {
            if self.in_cooldown() {
                return Err(ProcessKeyError::Unavailable);
            }
            let _daemon_id = self.ensure_daemon_session(handle)?;
            match self.transport.process_key() {
                Ok(actions) if actions.iter().any(|a| a.kind == ACTION_SESSION_INVALID) => {
                    match self.recreate_daemon_session(handle) {
                        Some(_) => match self.transport.process_key() {
                            Ok(a) => { self.mark_recovered(); Ok(a) }
                            Err(_) => { self.mark_failed(); Err(ProcessKeyError::Unavailable) }
                        },
                        None => { self.mark_failed(); Err(ProcessKeyError::Unavailable) }
                    }
                }
                Ok(actions) => { self.mark_recovered(); Ok(actions) }
                // Fail-fast: no recreate on plain D-Bus error.
                Err(_) => { self.mark_failed(); Err(ProcessKeyError::Unavailable) }
            }
        }

        fn daemon_session_id(&self, handle: LocalHandle) -> SessionId {
            self.sessions.lock().unwrap().get(&handle).map(|e| e.daemon_session_id).unwrap_or(0)
        }
    }

    fn passthrough_action() -> Vec<IpcAction> {
        vec![IpcAction {
            kind: ACTION_PASSTHROUGH,
            text: String::new(),
            cursor: 0,
            candidates: vec![],
            focused: 0,
        }]
    }

    fn session_invalid_action() -> Vec<IpcAction> {
        vec![IpcAction {
            kind: ACTION_SESSION_INVALID,
            text: String::new(),
            cursor: 0,
            candidates: vec![],
            focused: 0,
        }]
    }

    fn ok_action() -> Vec<IpcAction> {
        vec![IpcAction {
            kind: crate::ACTION_COMMIT,
            text: "あ".into(),
            cursor: 0,
            candidates: vec![],
            focused: 0,
        }]
    }

    fn dbus_err() -> zbus::Result<Vec<IpcAction>> {
        Err(zbus::Error::InputOutput(std::sync::Arc::new(
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken"),
        )))
    }

    // ── test cases ────────────────────────────────────────────────────────────

    #[test]
    fn test_session_invalid_triggers_recreate_and_retry() {
        // First process_key → SESSION_INVALID; recreate (id 2); retry → ok_action.
        let t = MockTransport::new(
            vec![Ok(1), Ok(2)],
            vec![Ok(session_invalid_action()), Ok(ok_action())],
        );
        let c = MockClient::new(t);
        let h = c.create_handle("test");
        let result = c.process_key(h).unwrap();
        assert!(result.iter().any(|a| a.kind == crate::ACTION_COMMIT));
        assert!(!c.in_cooldown());
    }

    #[test]
    fn test_retry_failure_arms_cooldown() {
        // SESSION_INVALID; recreate succeeds (id 2); retry also fails → Unavailable + cooldown.
        let t = MockTransport::new(
            vec![Ok(1), Ok(2)],
            vec![Ok(session_invalid_action()), dbus_err()],
        );
        let c = MockClient::new(t);
        let h = c.create_handle("test");
        let result = c.process_key(h);
        assert!(matches!(result, Err(ProcessKeyError::Unavailable)));
        assert!(c.in_cooldown());
    }

    #[test]
    fn test_cooldown_skips_dbus_call() {
        // Arm cooldown manually, then call process_key — should return Unavailable
        // without touching the transport (which has no queued responses).
        let t = MockTransport::new(vec![Ok(1)], vec![]);
        let c = MockClient::new(t);
        let h = c.create_handle("test");
        c.mark_failed();
        let result = c.process_key(h);
        assert!(matches!(result, Err(ProcessKeyError::Unavailable)));
    }

    #[test]
    fn test_normal_success_clears_failure() {
        let t = MockTransport::new(vec![Ok(1)], vec![Ok(passthrough_action())]);
        let c = MockClient::new(t);
        let h = c.create_handle("test");
        // Manually clear cooldown (simulating time passing) so the call proceeds.
        c.mark_recovered();
        let result = c.process_key(h).unwrap();
        assert!(result.iter().any(|a| a.kind == ACTION_PASSTHROUGH));
        assert!(!c.in_cooldown());
    }

    #[test]
    fn test_unknown_handle_returns_error() {
        let t = MockTransport::new(vec![], vec![]);
        let c = MockClient::new(t);
        let result = c.process_key(999);
        assert!(matches!(result, Err(ProcessKeyError::UnknownHandle)));
    }

    /// A plain D-Bus error must fail immediately without calling create_session
    /// or process_key a second time (fail-fast path).
    #[test]
    fn test_dbus_err_fail_fast() {
        // Only one process_key response queued; if recreate were attempted it
        // would panic with "unexpected create_session call".
        let t = MockTransport::new(vec![Ok(1)], vec![dbus_err()]);
        let c = MockClient::new(t);
        let h = c.create_handle("test");
        let result = c.process_key(h);
        assert!(matches!(result, Err(ProcessKeyError::Unavailable)));
        assert!(c.in_cooldown());
        // All sessions should have been invalidated.
        assert_eq!(c.daemon_session_id(h), 0);
    }

    /// Repeated mark_failed calls should grow the consecutive_failures counter,
    /// yielding longer cooldowns.
    #[test]
    fn test_exponential_backoff_sequence() {
        let t = MockTransport::new(vec![], vec![]);
        let c = MockClient::new(t);
        assert_eq!(c.consecutive_failures(), 0);

        c.mark_failed();
        assert_eq!(c.consecutive_failures(), 1);
        assert_eq!(current_cooldown(1), INITIAL_COOLDOWN); // 2s

        c.mark_failed();
        assert_eq!(c.consecutive_failures(), 2);
        assert_eq!(current_cooldown(2), Duration::from_millis(4000)); // 4s

        c.mark_failed();
        assert_eq!(c.consecutive_failures(), 3);
        assert_eq!(current_cooldown(3), Duration::from_millis(8000)); // 8s
    }

    /// The cooldown must never exceed MAX_COOLDOWN regardless of failure count.
    #[test]
    fn test_cooldown_capped_at_max() {
        assert_eq!(current_cooldown(20), MAX_COOLDOWN);
        assert_eq!(current_cooldown(100), MAX_COOLDOWN);
    }

    /// mark_recovered must reset consecutive_failures to zero.
    #[test]
    fn test_mark_recovered_resets_counter() {
        let t = MockTransport::new(vec![], vec![]);
        let c = MockClient::new(t);
        c.mark_failed();
        c.mark_failed();
        c.mark_failed();
        assert_eq!(c.consecutive_failures(), 3);
        c.mark_recovered();
        assert_eq!(c.consecutive_failures(), 0);
        assert!(!c.in_cooldown());
    }

    /// create_handle during cooldown must not block (no D-Bus call).
    #[test]
    fn test_create_handle_respects_cooldown() {
        // No session_ids queued; if create_session were called it would panic.
        let t = MockTransport::new(vec![], vec![]);
        let c = MockClient::new(t);
        c.mark_failed(); // arm cooldown
        let h = c.create_handle("test");
        // Handle should be registered with daemon_session_id == 0.
        assert_eq!(c.daemon_session_id(h), 0);
    }
}
