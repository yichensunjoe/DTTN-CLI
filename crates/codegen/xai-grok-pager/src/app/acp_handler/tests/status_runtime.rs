use super::*;
use xai_grok_shell::session::status_runtime_snapshot::{
    StatusRunState, StatusRuntimeNotification, StatusRuntimeWireSnapshot,
};

fn status_notification(session_id: &str, revision: u64, model_id: &str) -> acp::ExtNotification {
    let payload = StatusRuntimeNotification {
        session_id: acp::SessionId::new(session_id.to_owned()),
        status: StatusRuntimeWireSnapshot {
            revision,
            run_state: StatusRunState::Running,
            model_id: model_id.to_owned(),
            context_window: 200_000,
            active_tools: 2,
            pending_interactions: 1,
            ..Default::default()
        },
    };
    acp::ExtNotification::new(
        "x.ai/status_runtime",
        std::sync::Arc::from(serde_json::value::to_raw_value(&payload).unwrap()),
    )
}

#[test]
fn live_status_notification_updates_matching_session() {
    let mut app = make_app_with_agent("sess-1");
    let notification = status_notification("sess-1", 4, "openai/gpt-5");

    assert!(handle_ext_notification(&notification, &mut app));

    let status = app.agents[&AgentId(0)]
        .status_runtime
        .as_ref()
        .expect("runtime status stored");
    assert_eq!(status.revision, 4);
    assert_eq!(status.model_id, "openai/gpt-5");
    assert_eq!(status.active_tools, 2);
    assert_eq!(status.pending_interactions, 1);
}

#[test]
fn stale_and_equal_status_revisions_are_ignored() {
    let mut app = make_app_with_agent("sess-1");
    assert!(handle_ext_notification(
        &status_notification("sess-1", 8, "new-model"),
        &mut app,
    ));

    assert!(!handle_ext_notification(
        &status_notification("sess-1", 8, "duplicate-model"),
        &mut app,
    ));
    assert!(!handle_ext_notification(
        &status_notification("sess-1", 7, "stale-model"),
        &mut app,
    ));

    let status = app.agents[&AgentId(0)].status_runtime.as_ref().unwrap();
    assert_eq!(status.revision, 8);
    assert_eq!(status.model_id, "new-model");
}

#[test]
fn status_notification_for_unknown_session_is_dropped() {
    let mut app = make_app_with_agent("sess-1");
    assert!(!handle_ext_notification(
        &status_notification("other-session", 1, "other-model"),
        &mut app,
    ));
    assert!(app.agents[&AgentId(0)].status_runtime.is_none());
}
