//! Lock-bounded, I/O-free runtime status snapshots for TUI and leader clients.
//!
//! Writers publish complete immutable snapshots after runtime events. Readers only
//! clone an `Arc` and never perform network, filesystem, Git, or async work.

use std::sync::{Arc, RwLock};

use super::session_model_snapshot::ResolvedSessionModelSnapshot;

/// Coarse execution state rendered by status surfaces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusRunState {
    #[default]
    Idle,
    Running,
    WaitingForInput,
    Cancelling,
    Failed,
}

/// Session-frozen model contract exposed to status consumers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusModelContract {
    pub model_id: String,
    pub context_window: u64,
    pub max_completion_tokens: Option<u32>,
    pub catalog_origin: Option<String>,
    pub catalog_revision: Option<String>,
    pub catalog_stale: bool,
    pub currency: Option<String>,
    /// Provider currency micro-units per one million input tokens.
    pub input_per_million_microunits: Option<u64>,
    /// Provider currency micro-units per one million cached-input tokens.
    pub cached_input_per_million_microunits: Option<u64>,
    /// Provider currency micro-units per one million output tokens.
    pub output_per_million_microunits: Option<u64>,
}

impl From<&ResolvedSessionModelSnapshot> for StatusModelContract {
    fn from(snapshot: &ResolvedSessionModelSnapshot) -> Self {
        let pricing = snapshot
            .catalog_metadata
            .as_ref()
            .map(|metadata| &metadata.pricing);
        Self {
            model_id: snapshot.model_id.clone(),
            context_window: snapshot.context_window,
            max_completion_tokens: snapshot.max_completion_tokens,
            catalog_origin: snapshot.catalog_origin.clone(),
            catalog_revision: snapshot.catalog_revision.clone(),
            catalog_stale: snapshot.catalog_stale,
            currency: pricing
                .and_then(|pricing| pricing.currency.as_ref())
                .map(|value| value.value.clone()),
            input_per_million_microunits: pricing
                .and_then(|pricing| pricing.input_per_million_microunits.as_ref())
                .map(|value| value.value),
            cached_input_per_million_microunits: pricing
                .and_then(|pricing| pricing.cached_input_per_million_microunits.as_ref())
                .map(|value| value.value),
            output_per_million_microunits: pricing
                .and_then(|pricing| pricing.output_per_million_microunits.as_ref())
                .map(|value| value.value),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusTokenUsage {
    pub turn_input: u64,
    pub turn_output: u64,
    pub session_input: u64,
    pub session_output: u64,
    pub cached_input: u64,
}

impl StatusTokenUsage {
    pub fn session_total(self) -> u64 {
        self.session_input.saturating_add(self.session_output)
    }

    pub fn context_percent(self, context_window: u64) -> Option<u8> {
        if context_window == 0 {
            return None;
        }
        let percent = self
            .session_total()
            .saturating_mul(100)
            .checked_div(context_window)
            .unwrap_or(100)
            .min(100);
        Some(percent as u8)
    }
}

/// Cost values use integer provider-currency micro-units to avoid drift.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusCost {
    pub currency: Option<String>,
    pub turn_microunits: Option<u64>,
    pub session_microunits: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusLatency {
    pub last_request_ms: Option<u64>,
    pub time_to_first_token_ms: Option<u64>,
    pub last_tool_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusToolActivity {
    pub active_count: u32,
    pub queued_count: u32,
    pub last_tool_name: Option<String>,
}

/// Complete immutable view consumed by the status line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusRuntimeSnapshot {
    /// Monotonic publication sequence. Consumers can skip duplicate renders.
    pub revision: u64,
    pub run_state: StatusRunState,
    /// Ownership token for lifecycle publications. Old completion or cancel
    /// events must not overwrite a newer turn's state.
    pub active_prompt_id: Option<String>,
    pub model: StatusModelContract,
    pub tokens: StatusTokenUsage,
    pub cost: StatusCost,
    pub latency: StatusLatency,
    pub tools: StatusToolActivity,
    pub pending_interactions: u32,
}

impl StatusRuntimeSnapshot {
    pub fn from_session_model(snapshot: &ResolvedSessionModelSnapshot) -> Self {
        Self {
            model: StatusModelContract::from(snapshot),
            ..Self::default()
        }
    }
}

#[derive(Debug)]
struct StatusRuntimeInner {
    current: RwLock<Arc<StatusRuntimeSnapshot>>,
}

/// Cloneable publication handle shared by SessionActor, SessionHandle and TUI.
#[derive(Debug, Clone)]
pub struct StatusRuntimePublisher {
    inner: Arc<StatusRuntimeInner>,
}

impl StatusRuntimePublisher {
    pub fn new(initial: StatusRuntimeSnapshot) -> Self {
        Self {
            inner: Arc::new(StatusRuntimeInner {
                current: RwLock::new(Arc::new(initial)),
            }),
        }
    }

    /// Read path used by renderers. It performs no I/O and no async work.
    pub fn snapshot(&self) -> Arc<StatusRuntimeSnapshot> {
        self.inner
            .current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Publish a complete new immutable generation.
    ///
    /// The write lock covers the complete read-modify-publish transaction. This
    /// prevents concurrent runtime event publishers from cloning the same stale
    /// generation and silently overwriting each other's fields.
    pub fn update(
        &self,
        mutate: impl FnOnce(&mut StatusRuntimeSnapshot),
    ) -> Arc<StatusRuntimeSnapshot> {
        let mut current = self
            .inner
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = (**current).clone();
        mutate(&mut next);
        next.revision = next.revision.saturating_add(1);
        let next = Arc::new(next);
        *current = next.clone();
        next
    }

    /// Publish only when `mutate` accepts the current generation. Rejected
    /// stale lifecycle events neither replace the snapshot nor increment its
    /// revision.
    fn update_if(
        &self,
        mutate: impl FnOnce(&mut StatusRuntimeSnapshot) -> bool,
    ) -> Option<Arc<StatusRuntimeSnapshot>> {
        let mut current = self
            .inner
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = (**current).clone();
        if !mutate(&mut next) {
            return None;
        }
        next.revision = next.revision.saturating_add(1);
        let next = Arc::new(next);
        *current = next.clone();
        Some(next)
    }

    /// Start a prompt-owned turn and clear only turn-scoped measurements.
    pub fn begin_turn(&self, prompt_id: impl Into<String>) -> Arc<StatusRuntimeSnapshot> {
        let prompt_id = prompt_id.into();
        self.update(|snapshot| {
            snapshot.active_prompt_id = Some(prompt_id);
            snapshot.run_state = StatusRunState::Running;
            snapshot.tokens.turn_input = 0;
            snapshot.tokens.turn_output = 0;
            snapshot.cost.turn_microunits = None;
            snapshot.latency = StatusLatency::default();
            snapshot.tools.active_count = 0;
            snapshot.tools.last_tool_name = None;
        })
    }

    /// Mark cancellation only when the event still owns the active prompt.
    pub fn mark_cancelling(&self, prompt_id: &str) -> Option<Arc<StatusRuntimeSnapshot>> {
        self.update_if(|snapshot| {
            if snapshot.active_prompt_id.as_deref() != Some(prompt_id) {
                return false;
            }
            snapshot.run_state = StatusRunState::Cancelling;
            true
        })
    }

    /// Finish only the matching active prompt. `run_state` is normally `Idle`
    /// for completed/cancelled turns or `Failed` for terminal errors/limits.
    pub fn finish_turn(
        &self,
        prompt_id: &str,
        run_state: StatusRunState,
    ) -> Option<Arc<StatusRuntimeSnapshot>> {
        debug_assert!(matches!(run_state, StatusRunState::Idle | StatusRunState::Failed));
        self.update_if(|snapshot| {
            if snapshot.active_prompt_id.as_deref() != Some(prompt_id) {
                return false;
            }
            snapshot.active_prompt_id = None;
            snapshot.run_state = run_state;
            snapshot.tools.active_count = 0;
            true
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_observe_complete_generations() {
        let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
        let published = publisher.update(|snapshot| {
            snapshot.run_state = StatusRunState::Running;
            snapshot.tokens.session_input = 42;
            snapshot.tools.active_count = 1;
        });
        let read = publisher.snapshot();
        assert_eq!(published.revision, 1);
        assert_eq!(read.revision, 1);
        assert_eq!(read.run_state, StatusRunState::Running);
        assert_eq!(read.tokens.session_input, 42);
        assert_eq!(read.tools.active_count, 1);
    }

    #[test]
    fn concurrent_writers_do_not_lose_runtime_updates() {
        const WRITERS: usize = 8;
        const UPDATES_PER_WRITER: usize = 250;

        let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
        let threads = (0..WRITERS)
            .map(|_| {
                let publisher = publisher.clone();
                std::thread::spawn(move || {
                    for _ in 0..UPDATES_PER_WRITER {
                        publisher.update(|snapshot| {
                            snapshot.tokens.session_input =
                                snapshot.tokens.session_input.saturating_add(1);
                        });
                    }
                })
            })
            .collect::<Vec<_>>();

        for thread in threads {
            thread.join().expect("status publisher writer panicked");
        }

        let snapshot = publisher.snapshot();
        let expected = (WRITERS * UPDATES_PER_WRITER) as u64;
        assert_eq!(snapshot.tokens.session_input, expected);
        assert_eq!(snapshot.revision, expected);
    }

    #[test]
    fn lifecycle_publications_are_prompt_owned() {
        let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
        publisher.begin_turn("turn-a");
        publisher.begin_turn("turn-b");

        let revision = publisher.snapshot().revision;
        assert!(publisher.mark_cancelling("turn-a").is_none());
        assert!(
            publisher
                .finish_turn("turn-a", StatusRunState::Idle)
                .is_none()
        );
        let active = publisher.snapshot();
        assert_eq!(active.revision, revision);
        assert_eq!(active.active_prompt_id.as_deref(), Some("turn-b"));
        assert_eq!(active.run_state, StatusRunState::Running);

        assert!(publisher.mark_cancelling("turn-b").is_some());
        assert_eq!(publisher.snapshot().run_state, StatusRunState::Cancelling);
        assert!(
            publisher
                .finish_turn("turn-b", StatusRunState::Idle)
                .is_some()
        );
        let idle = publisher.snapshot();
        assert_eq!(idle.active_prompt_id, None);
        assert_eq!(idle.run_state, StatusRunState::Idle);
    }

    #[test]
    fn begin_turn_resets_only_turn_scoped_measurements() {
        let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot {
            tokens: StatusTokenUsage {
                turn_input: 10,
                turn_output: 20,
                session_input: 100,
                session_output: 200,
                cached_input: 30,
            },
            cost: StatusCost {
                currency: Some("USD".to_string()),
                turn_microunits: Some(10),
                session_microunits: Some(50),
            },
            latency: StatusLatency {
                last_request_ms: Some(100),
                time_to_first_token_ms: Some(20),
                last_tool_ms: Some(5),
            },
            tools: StatusToolActivity {
                active_count: 2,
                queued_count: 3,
                last_tool_name: Some("bash".to_string()),
            },
            ..Default::default()
        });

        let running = publisher.begin_turn("turn-a");
        assert_eq!(running.tokens.turn_input, 0);
        assert_eq!(running.tokens.turn_output, 0);
        assert_eq!(running.tokens.session_input, 100);
        assert_eq!(running.tokens.session_output, 200);
        assert_eq!(running.tokens.cached_input, 30);
        assert_eq!(running.cost.turn_microunits, None);
        assert_eq!(running.cost.session_microunits, Some(50));
        assert_eq!(running.latency, StatusLatency::default());
        assert_eq!(running.tools.active_count, 0);
        assert_eq!(running.tools.queued_count, 3);
        assert_eq!(running.tools.last_tool_name, None);
    }

    #[test]
    fn context_percent_is_bounded_and_zero_safe() {
        let usage = StatusTokenUsage {
            session_input: 80,
            session_output: 40,
            ..Default::default()
        };
        assert_eq!(usage.context_percent(0), None);
        assert_eq!(usage.context_percent(200), Some(60));
        assert_eq!(usage.context_percent(100), Some(100));
    }
}
