//! Per-session pending-interaction registry.
//!
//! Permissions, `ask_user_question`, and plan approval are **blocking ACP
//! reverse-requests**: the agent parks a tool-loop future on an in-memory
//! oneshot and waits for the driver to answer. While such a request is open we
//! record it here, keyed by `tool_call_id` (stable, lives in the transcript →
//! survives reconnect). This registry is the single source of truth for "what
//! is pending right now" and is read by the roster to surface
//! [`crate::agent::roster::RosterActivity::NeedsInput`].
//!
//! Pending interactions are **requests, not notifications** — they are never
//! persisted. We broadcast `pending_interaction` / `interaction_resolved`
//! **fire-and-forget** via the gateway (same idiom as
//! [`crate::session::summary`]); the routing layer fans them to every
//! subscriber because they carry a `sessionId`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use agent_client_protocol as acp;
use xai_acp_lib::AcpAgentGatewaySender as GatewaySender;

use crate::extensions::notification::{SessionNotification, SessionUpdate as XaiSessionUpdate};
use crate::session::status_runtime_snapshot::{
    StatusRunState, StatusRuntimeNotification, StatusRuntimePublisher, StatusRuntimeSnapshot,
    StatusRuntimeWireSnapshot,
};

/// Shared per-session map of open reverse-requests, keyed by `tool_call_id`.
///
/// Mirrors the `current_prompt_id` signal on
/// [`crate::session::handle::SessionHandle`]: the same `Arc` is shared between
/// the session actor (which mutates it) and the handle (which the roster reads
/// synchronously).
pub type PendingInteractions = Arc<Mutex<HashMap<String, PendingKind>>>;

/// Which kind of blocking reverse-request is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingKind {
    /// `request_permission` for a tool action.
    Permission,
    /// `x.ai/ask_user_question`.
    Question,
    /// `x.ai/exit_plan_mode` plan approval.
    PlanApproval,
}

/// Whether a blocking plan-approval reverse-request is parked in `pending`.
///
/// The resume re-park issues `x.ai/exit_plan_mode` from a detached task
/// with no running turn, making it the one parked interaction that also carries a
/// persisted gate (`awaiting_plan_approval`). `session_has_live_work` consults
/// this to keep such a session resident until the decision is answered or a real
/// disconnect `Err`s the reverse-request — otherwise an idle-unload drops the
/// parked future and its guard clears the on-disk gate. Permission/question parks
/// carry no persisted gate, so they are intentionally not counted here. Poisoned
/// lock → recover the map (module idiom) and read it.
pub(crate) fn has_parked_plan_approval(pending: &PendingInteractions) -> bool {
    pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .any(|k| *k == PendingKind::PlanApproval)
}

/// Fire-and-forget broadcast of a session notification carrying a `sessionId`
/// (so the routing layer fans it out to every subscriber). Never persisted.
fn broadcast(gateway: &GatewaySender, session_id: &acp::SessionId, update: XaiSessionUpdate) {
    let notification = SessionNotification {
        session_id: session_id.clone(),
        update,
        meta: None,
    };
    if let Ok(params) = serde_json::value::to_raw_value(&notification) {
        gateway.forward_fire_and_forget(acp::ExtNotification::new(
            "x.ai/session_notification",
            params.into(),
        ));
    }
}

pub(crate) const STATUS_RUNTIME_NOTIFICATION_METHOD: &str = "x.ai/status_runtime";

/// Start the one coalescing, non-blocking status notification bridge for a session.
///
/// The bridge is lazy: `x.ai/session/info` supplies the initial snapshot and
/// claims this task for clients that need live updates. It exits when all
/// publisher handles are dropped.
pub(crate) fn ensure_status_runtime_bridge(
    publisher: &StatusRuntimePublisher,
    gateway: GatewaySender,
    session_id: acp::SessionId,
) {
    if !publisher.claim_notification_bridge() {
        return;
    }
    let receiver = publisher.subscribe();
    tokio::task::spawn_local(run_status_runtime_bridge(receiver, gateway, session_id));
}

async fn run_status_runtime_bridge(
    mut receiver: watch::Receiver<Arc<StatusRuntimeSnapshot>>,
    gateway: GatewaySender,
    session_id: acp::SessionId,
) {
    let mut last_revision = None;
    loop {
        let snapshot = receiver.borrow_and_update().clone();
        let revision = snapshot.revision;
        if last_revision.map_or(true, |last| revision > last) {
            forward_status_snapshot(&gateway, &session_id, snapshot);
            last_revision = Some(revision);
        }
        if receiver.changed().await.is_err() {
            break;
        }
    }
}

fn forward_status_snapshot(
    gateway: &GatewaySender,
    session_id: &acp::SessionId,
    snapshot: Arc<StatusRuntimeSnapshot>,
) {
    let notification = StatusRuntimeNotification {
        session_id: session_id.clone(),
        status: StatusRuntimeWireSnapshot::from(snapshot),
    };
    match serde_json::value::to_raw_value(&notification) {
        Ok(params) => {
            gateway.forward_fire_and_forget(acp::ExtNotification::new(
                STATUS_RUNTIME_NOTIFICATION_METHOD,
                params.into(),
            ));
        }
        Err(error) => tracing::warn!(
            session_id = %session_id.0,
            %error,
            "failed to serialize status runtime notification"
        ),
    }
}

fn publish_pending_status(status_runtime: &StatusRuntimePublisher, count: usize) {
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    status_runtime.update(|snapshot| {
        snapshot.pending_interactions = count;
        if count > 0 {
            if !matches!(
                snapshot.run_state,
                StatusRunState::Cancelling | StatusRunState::Failed
            ) {
                snapshot.run_state = StatusRunState::WaitingForInput;
            }
        } else if snapshot.run_state == StatusRunState::WaitingForInput {
            snapshot.run_state = if snapshot.active_prompt_id.is_some() {
                StatusRunState::Running
            } else {
                StatusRunState::Idle
            };
        }
    });
}

/// RAII guard registering an open reverse-request for the lifetime of the
/// parked oneshot.
///
/// On construction it inserts `(tool_call_id, kind)` into the registry and
/// broadcasts `pending_interaction`. On drop — which happens whether the await
/// returns normally, is cancelled, or errors — it removes the entry and (if it
/// actually removed one) broadcasts `interaction_resolved`. The
/// remove-or-no-op makes resolution **idempotent / first-answer-wins**: a
/// second drop / already-removed key is silent.
pub struct PendingInteractionGuard {
    pending: PendingInteractions,
    status_runtime: StatusRuntimePublisher,
    gateway: GatewaySender,
    session_id: acp::SessionId,
    tool_call_id: String,
}

impl PendingInteractionGuard {
    /// Register a pending interaction and broadcast `pending_interaction`.
    pub fn new(
        pending: PendingInteractions,
        status_runtime: StatusRuntimePublisher,
        gateway: GatewaySender,
        session_id: acp::SessionId,
        tool_call_id: String,
        kind: PendingKind,
    ) -> Self {
        let count = {
            let mut map = pending.lock().unwrap_or_else(|e| e.into_inner());
            map.insert(tool_call_id.clone(), kind);
            map.len()
        };
        publish_pending_status(&status_runtime, count);
        broadcast(
            &gateway,
            &session_id,
            XaiSessionUpdate::PendingInteraction {
                tool_call_id: tool_call_id.clone(),
                kind,
            },
        );
        Self {
            pending,
            status_runtime,
            gateway,
            session_id,
            tool_call_id,
        }
    }
}

impl Drop for PendingInteractionGuard {
    fn drop(&mut self) {
        let removed_and_count = {
            let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            map.remove(&self.tool_call_id)
                .is_some()
                .then_some(map.len())
        };
        // First-answer-wins: only announce resolution if this guard actually
        // owned the live entry. An already-resolved id is a silent no-op.
        if let Some(count) = removed_and_count {
            publish_pending_status(&self.status_runtime, count);
            broadcast(
                &self.gateway,
                &self.session_id,
                XaiSessionUpdate::InteractionResolved {
                    tool_call_id: self.tool_call_id.clone(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_registry() -> PendingInteractions {
        Arc::new(Mutex::new(HashMap::new()))
    }

    fn new_status() -> StatusRuntimePublisher {
        StatusRuntimePublisher::new(StatusRuntimeSnapshot::default())
    }

    #[test]
    fn guard_inserts_then_removes_and_publishes_status() {
        let reg = new_registry();
        let status = new_status();
        status.begin_turn("turn-1");
        // No gateway round-trip is exercised here (broadcast is best-effort and
        // a dead sender simply drops). We only assert registry/status mutation.
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let gateway = GatewaySender::new(tx);
        {
            let _g = PendingInteractionGuard::new(
                reg.clone(),
                status.clone(),
                gateway,
                acp::SessionId::new("sess-1"),
                "call-1".to_string(),
                PendingKind::Permission,
            );
            assert_eq!(reg.lock().unwrap().len(), 1);
            assert_eq!(
                reg.lock().unwrap().get("call-1").copied(),
                Some(PendingKind::Permission)
            );
            let snapshot = status.snapshot();
            assert_eq!(snapshot.pending_interactions, 1);
            assert_eq!(snapshot.run_state, StatusRunState::WaitingForInput);
        }
        assert!(reg.lock().unwrap().is_empty());
        let snapshot = status.snapshot();
        assert_eq!(snapshot.pending_interactions, 0);
        assert_eq!(snapshot.run_state, StatusRunState::Running);
    }

    #[test]
    fn cancelling_state_is_not_overwritten_by_late_interaction_resolution() {
        let reg = new_registry();
        let status = new_status();
        status.begin_turn("turn-1");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let gateway = GatewaySender::new(tx);
        let guard = PendingInteractionGuard::new(
            reg,
            status.clone(),
            gateway,
            acp::SessionId::new("sess-1"),
            "call-1".to_string(),
            PendingKind::Question,
        );
        assert!(status.mark_cancelling("turn-1").is_some());
        drop(guard);
        let snapshot = status.snapshot();
        assert_eq!(snapshot.pending_interactions, 0);
        assert_eq!(snapshot.run_state, StatusRunState::Cancelling);
    }

    /// `has_parked_plan_approval` counts ONLY a parked plan-approval; other
    /// kinds (permission / question) carry no persisted gate and must not, by
    /// themselves, report the session live.
    #[test]
    fn has_parked_plan_approval_only_counts_plan_approval() {
        let reg = new_registry();
        assert!(!has_parked_plan_approval(&reg));

        reg.lock()
            .unwrap()
            .insert("perm".to_string(), PendingKind::Permission);
        reg.lock()
            .unwrap()
            .insert("q".to_string(), PendingKind::Question);
        assert!(
            !has_parked_plan_approval(&reg),
            "permission/question parks must not count as a parked approval"
        );

        reg.lock()
            .unwrap()
            .insert("plan".to_string(), PendingKind::PlanApproval);
        assert!(has_parked_plan_approval(&reg));

        reg.lock().unwrap().remove("plan");
        assert!(!has_parked_plan_approval(&reg));
    }

    /// A poisoned registry lock must not panic the predicate: it recovers the
    /// inner map (module idiom) and reports the parked approval truthfully.
    #[test]
    fn has_parked_plan_approval_recovers_poisoned_lock() {
        let reg = new_registry();
        reg.lock()
            .unwrap()
            .insert("plan".to_string(), PendingKind::PlanApproval);

        let reg_poison = reg.clone();
        let _ = std::thread::spawn(move || {
            let _g = reg_poison.lock().unwrap();
            panic!("poison pending_interactions");
        })
        .join();
        assert!(
            reg.lock().is_err(),
            "precondition: the lock must be poisoned"
        );

        assert!(
            has_parked_plan_approval(&reg),
            "a poisoned lock must still surface the parked approval, not panic"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_bridge_exits_after_publishers_drop() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let publisher = new_status();
                let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                let gateway = GatewaySender::new(tx);
                let receiver = publisher.subscribe();
                let task = tokio::task::spawn_local(run_status_runtime_bridge(
                    receiver,
                    gateway,
                    acp::SessionId::new("sess-bridge"),
                ));
                drop(publisher);
                tokio::time::timeout(std::time::Duration::from_secs(1), task)
                    .await
                    .expect("bridge exits when watch closes")
                    .expect("bridge task does not panic");
            })
            .await;
    }
}
