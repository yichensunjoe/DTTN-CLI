use xai_grok_shell::session::status_runtime_snapshot::{
    StatusRunState, StatusRuntimePublisher, StatusRuntimeSnapshot, StatusTokenUsage,
};

#[test]
fn renderer_reads_complete_immutable_generations_without_async_work() {
    let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
    let before = publisher.snapshot();
    let after = publisher.update(|snapshot| {
        snapshot.run_state = StatusRunState::Running;
        snapshot.tokens.session_input = 120;
        snapshot.tokens.session_output = 30;
    });
    assert_eq!(before.revision, 0);
    assert_eq!(after.revision, 1);
    assert_eq!(after.tokens.session_total(), 150);
    assert_eq!(publisher.snapshot().revision, 1);
}

#[test]
fn context_utilization_is_saturating_and_unknown_safe() {
    let usage = StatusTokenUsage {
        session_input: 90,
        session_output: 30,
        ..Default::default()
    };
    assert_eq!(usage.context_percent(0), None);
    assert_eq!(usage.context_percent(240), Some(50));
    assert_eq!(usage.context_percent(100), Some(100));
}
