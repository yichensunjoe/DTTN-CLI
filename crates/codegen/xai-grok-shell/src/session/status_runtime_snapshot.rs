//! Lock-bounded, I/O-free runtime status snapshots for TUI and leader clients.
//!
//! Writers publish complete immutable snapshots after runtime events. Readers only
//! clone an `Arc` and never perform network, filesystem, Git, or async work.

use std::sync::{Arc, RwLock};
use std::time::Instant;

use super::session_model_snapshot::ResolvedSessionModelSnapshot;

const USD_TICKS_PER_MICROUNIT: u64 = 10_000;

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
    pub turn_cached_input: u64,
    pub session_input: u64,
    pub session_output: u64,
    pub session_cached_input: u64,
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

/// Trusted server-billed cost values expressed as USD micro-units.
///
/// Model-catalog prices retain their native provider currency in
/// [`StatusModelContract`]. Runtime cost is USD because the sampler ledger's
/// authoritative cost contract is USD ticks (1e10 ticks per USD).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusCost {
    pub currency: Option<String>,
    pub turn_microunits: Option<u64>,
    pub session_microunits: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusLatency {
    /// Aggregate provider API duration for the completed turn.
    pub turn_api_duration_ms: Option<u64>,
    /// Absolute aggregate provider API duration for the restored/current session.
    pub session_api_duration_ms: Option<u64>,
    /// Last individual inference request duration; populated by sampler events later.
    pub last_request_ms: Option<u64>,
    pub time_to_first_token_ms: Option<u64>,
    pub last_tool_ms: Option<u64>,
}

/// Normalized usage publication produced from the prompt/session billing ledgers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusUsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_read_tokens: u64,
    pub model_calls: u64,
    pub api_duration_ms: u64,
    pub cost_usd_ticks: Option<i64>,
    /// False when the ledger is incomplete or its cost is partial.
    pub cost_trusted: bool,
}

impl StatusUsageTotals {
    pub fn from_prompt_usage(usage: &crate::extensions::notification::PromptUsage) -> Self {
        let totals = &usage.totals;
        Self {
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cached_read_tokens: totals.cached_read_tokens,
            model_calls: totals.model_calls,
            api_duration_ms: totals.api_duration_ms,
            cost_usd_ticks: totals.cost_usd_ticks,
            cost_trusted: !usage.usage_is_incomplete && !totals.cost_is_partial,
        }
    }

    pub fn from_session_ledger(ledger: &xai_chat_state::UsageLedger) -> Self {
        let totals = &ledger.totals;
        Self {
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cached_read_tokens: totals.cached_read_tokens,
            model_calls: totals.model_calls,
            api_duration_ms: totals.api_duration_ms,
            cost_usd_ticks: totals.cost_usd_ticks,
            cost_trusted: !ledger.incomplete && !totals.cost_is_partial(),
        }
    }

    fn trusted_cost_microunits(self) -> Option<u64> {
        if !self.cost_trusted {
            return None;
        }
        let ticks = u64::try_from(self.cost_usd_ticks?).ok()?;
        Some(ticks / USD_TICKS_PER_MICROUNIT)
    }

    fn api_duration(self) -> Option<u64> {
        (self.model_calls > 0).then_some(self.api_duration_ms)
    }
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

/// Prompt-owned RAII guard for one concurrently executing tool.
/// Cancellation and task aborts drop the guard, so active counts cannot
/// remain stuck. A guard from an older prompt is ignored after a new
/// prompt owns the snapshot.
#[must_use]
pub struct StatusToolGuard {
    publisher: StatusRuntimePublisher,
    prompt_id: Option<String>,
    tool_name: String,
    started_at: Instant,
    finished: bool,
}

impl StatusToolGuard {
    pub fn finish(mut self) {
        self.publish_finished();
        self.finished = true;
    }

    fn publish_finished(&self) {
        let Some(prompt_id) = self.prompt_id.as_deref() else {
            return;
        };
        let duration_ms = self.started_at.elapsed().as_millis() as u64;
        self.publisher.update_if(|snapshot| {
            if snapshot.active_prompt_id.as_deref() != Some(prompt_id) {
                return false;
            }
            snapshot.tools.active_count = snapshot.tools.active_count.saturating_sub(1);
            snapshot.tools.last_tool_name = Some(self.tool_name.clone());
            snapshot.latency.last_tool_ms = Some(duration_ms);
            true
        });
    }
}

impl Drop for StatusToolGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.publish_finished();
        }
    }
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
            snapshot.tokens.turn_cached_input = 0;
            snapshot.cost.turn_microunits = None;
            snapshot.latency.turn_api_duration_ms = None;
            snapshot.latency.last_request_ms = None;
            snapshot.latency.time_to_first_token_ms = None;
            snapshot.latency.last_tool_ms = None;
            snapshot.tools.active_count = 0;
            snapshot.tools.queued_count = 0;
            snapshot.tools.last_tool_name = None;
        })
    }

    /// Publish a completed turn ledger plus an absolute session ledger.
    ///
    /// Session totals are replaced rather than incremented so resumed sessions
    /// preserve their complete historical usage and duplicate terminal events
    /// cannot double-count. If the session ledger is temporarily unavailable,
    /// the last known absolute session values remain unchanged.
    pub fn publish_usage(
        &self,
        turn: Option<StatusUsageTotals>,
        session: Option<StatusUsageTotals>,
    ) -> Arc<StatusRuntimeSnapshot> {
        self.update(|snapshot| {
            if let Some(turn) = turn {
                snapshot.tokens.turn_input = turn.input_tokens;
                snapshot.tokens.turn_output = turn.output_tokens;
                snapshot.tokens.turn_cached_input = turn.cached_read_tokens;
                snapshot.cost.turn_microunits = turn.trusted_cost_microunits();
                snapshot.latency.turn_api_duration_ms = turn.api_duration();
            }

            if let Some(session) = session {
                snapshot.tokens.session_input = session.input_tokens;
                snapshot.tokens.session_output = session.output_tokens;
                snapshot.tokens.session_cached_input = session.cached_read_tokens;
                snapshot.cost.session_microunits = session.trusted_cost_microunits();
                snapshot.latency.session_api_duration_ms = session.api_duration();
            }

            snapshot.cost.currency = (snapshot.cost.turn_microunits.is_some()
                || snapshot.cost.session_microunits.is_some())
            .then(|| "USD".to_string());
        })
    }

    /// Replace the number of approved tool calls waiting to be polled.
    pub fn queue_tools(&self, count: usize) -> Option<Arc<StatusRuntimeSnapshot>> {
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        self.update_if(|snapshot| {
            if snapshot.active_prompt_id.is_none() || snapshot.tools.queued_count == count {
                return false;
            }
            snapshot.tools.queued_count = count;
            true
        })
    }

    /// Start one real dispatch future and return an abort-safe guard.
    pub fn begin_tool(&self, tool_name: impl Into<String>) -> StatusToolGuard {
        let tool_name = tool_name.into();
        let mut prompt_id = None;
        self.update_if(|snapshot| {
            let Some(active_prompt_id) = snapshot.active_prompt_id.as_ref() else {
                return false;
            };
            prompt_id = Some(active_prompt_id.clone());
            snapshot.tools.queued_count = snapshot.tools.queued_count.saturating_sub(1);
            snapshot.tools.active_count = snapshot.tools.active_count.saturating_add(1);
            snapshot.tools.last_tool_name = Some(tool_name.clone());
            true
        });
        StatusToolGuard {
            publisher: self.clone(),
            prompt_id,
            tool_name,
            started_at: Instant::now(),
            finished: false,
        }
    }

    /// Mark cancellation only when the event still owns the active prompt.
    pub fn mark_cancelling(&self, prompt_id: &str) -> Option<Arc<StatusRuntimeSnapshot>> {
        self.update_if(|snapshot| {
            if snapshot.active_prompt_id.as_deref() != Some(prompt_id) {
                return false;
            }
            snapshot.run_state = StatusRunState::Cancelling;
            snapshot.tools.queued_count = 0;
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
        debug_assert!(matches!(
            run_state,
            StatusRunState::Idle | StatusRunState::Failed
        ));
        self.update_if(|snapshot| {
            if snapshot.active_prompt_id.as_deref() != Some(prompt_id) {
                return false;
            }
            snapshot.active_prompt_id = None;
            snapshot.run_state = run_state;
            snapshot.tools.active_count = 0;
            snapshot.tools.queued_count = 0;
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
                turn_cached_input: 5,
                session_input: 100,
                session_output: 200,
                session_cached_input: 30,
            },
            cost: StatusCost {
                currency: Some("USD".to_string()),
                turn_microunits: Some(10),
                session_microunits: Some(50),
            },
            latency: StatusLatency {
                turn_api_duration_ms: Some(80),
                session_api_duration_ms: Some(500),
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
        assert_eq!(running.tokens.turn_cached_input, 0);
        assert_eq!(running.tokens.session_input, 100);
        assert_eq!(running.tokens.session_output, 200);
        assert_eq!(running.tokens.session_cached_input, 30);
        assert_eq!(running.cost.turn_microunits, None);
        assert_eq!(running.cost.session_microunits, Some(50));
        assert_eq!(running.latency.turn_api_duration_ms, None);
        assert_eq!(running.latency.session_api_duration_ms, Some(500));
        assert_eq!(running.latency.last_request_ms, None);
        assert_eq!(running.latency.time_to_first_token_ms, None);
        assert_eq!(running.latency.last_tool_ms, None);
        assert_eq!(running.tools.active_count, 0);
        assert_eq!(running.tools.queued_count, 3);
        assert_eq!(running.tools.last_tool_name, None);
    }

    #[test]
    fn usage_publication_replaces_session_totals_and_fails_closed_on_cost() {
        let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
        let published = publisher.publish_usage(
            Some(StatusUsageTotals {
                input_tokens: 100,
                output_tokens: 20,
                cached_read_tokens: 40,
                model_calls: 2,
                api_duration_ms: 1250,
                cost_usd_ticks: Some(25_000),
                cost_trusted: true,
            }),
            Some(StatusUsageTotals {
                input_tokens: 1_000,
                output_tokens: 200,
                cached_read_tokens: 400,
                model_calls: 7,
                api_duration_ms: 5000,
                cost_usd_ticks: Some(999_999),
                cost_trusted: false,
            }),
        );

        assert_eq!(published.tokens.turn_input, 100);
        assert_eq!(published.tokens.turn_output, 20);
        assert_eq!(published.tokens.turn_cached_input, 40);
        assert_eq!(published.tokens.session_input, 1_000);
        assert_eq!(published.tokens.session_output, 200);
        assert_eq!(published.tokens.session_cached_input, 400);
        assert_eq!(published.cost.turn_microunits, Some(2));
        assert_eq!(published.cost.session_microunits, None);
        assert_eq!(published.cost.currency.as_deref(), Some("USD"));
        assert_eq!(published.latency.turn_api_duration_ms, Some(1250));
        assert_eq!(published.latency.session_api_duration_ms, Some(5000));
    }

    #[test]
    fn usage_publication_uses_absolute_session_values() {
        let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
        let first = StatusUsageTotals {
            input_tokens: 100,
            output_tokens: 50,
            model_calls: 1,
            ..Default::default()
        };
        publisher.publish_usage(Some(first), Some(first));

        let restored_absolute = StatusUsageTotals {
            input_tokens: 900,
            output_tokens: 300,
            cached_read_tokens: 250,
            model_calls: 5,
            api_duration_ms: 4000,
            ..Default::default()
        };
        let published = publisher.publish_usage(None, Some(restored_absolute));
        assert_eq!(published.tokens.session_input, 900);
        assert_eq!(published.tokens.session_output, 300);
        assert_eq!(published.tokens.session_cached_input, 250);
        assert_eq!(published.tokens.turn_input, 100);
        assert_eq!(published.tokens.turn_output, 50);
    }

    #[test]
    fn tool_guards_track_parallel_activity_and_latency() {
        let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
        publisher.begin_turn("turn-a");
        assert!(publisher.queue_tools(2).is_some());

        let first = publisher.begin_tool("bash");
        let second = publisher.begin_tool("read_file");
        let active = publisher.snapshot();
        assert_eq!(active.tools.queued_count, 0);
        assert_eq!(active.tools.active_count, 2);

        first.finish();
        assert_eq!(publisher.snapshot().tools.active_count, 1);
        drop(second);
        let done = publisher.snapshot();
        assert_eq!(done.tools.active_count, 0);
        assert_eq!(done.tools.last_tool_name.as_deref(), Some("read_file"));
        assert!(done.latency.last_tool_ms.is_some());
    }

    #[test]
    fn stale_tool_guard_cannot_overwrite_a_new_turn() {
        let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
        publisher.begin_turn("turn-a");
        publisher.queue_tools(1);
        let stale = publisher.begin_tool("bash");
        publisher.begin_turn("turn-b");
        let revision = publisher.snapshot().revision;
        drop(stale);

        let current = publisher.snapshot();
        assert_eq!(current.revision, revision);
        assert_eq!(current.active_prompt_id.as_deref(), Some("turn-b"));
        assert_eq!(current.tools.active_count, 0);
        assert_eq!(current.tools.queued_count, 0);
        assert_eq!(current.tools.last_tool_name, None);
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
