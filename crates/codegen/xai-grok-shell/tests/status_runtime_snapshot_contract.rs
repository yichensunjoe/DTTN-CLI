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
fn concurrent_event_publishers_preserve_every_update() {
    const WRITERS: usize = 6;
    const UPDATES_PER_WRITER: usize = 200;

    let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
    let writers = (0..WRITERS)
        .map(|_| {
            let publisher = publisher.clone();
            std::thread::spawn(move || {
                for _ in 0..UPDATES_PER_WRITER {
                    publisher.update(|snapshot| {
                        snapshot.tokens.session_output =
                            snapshot.tokens.session_output.saturating_add(1);
                    });
                }
            })
        })
        .collect::<Vec<_>>();

    for writer in writers {
        writer.join().expect("status runtime writer panicked");
    }

    let snapshot = publisher.snapshot();
    let expected = (WRITERS * UPDATES_PER_WRITER) as u64;
    assert_eq!(snapshot.tokens.session_output, expected);
    assert_eq!(snapshot.revision, expected);
}

#[test]
fn stale_turn_events_cannot_overwrite_the_active_turn() {
    let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
    publisher.begin_turn("turn-a");
    publisher.begin_turn("turn-b");

    let active_revision = publisher.snapshot().revision;
    assert!(publisher.mark_cancelling("turn-a").is_none());
    assert!(
        publisher
            .finish_turn("turn-a", StatusRunState::Failed)
            .is_none()
    );

    let active = publisher.snapshot();
    assert_eq!(active.revision, active_revision);
    assert_eq!(active.active_prompt_id.as_deref(), Some("turn-b"));
    assert_eq!(active.run_state, StatusRunState::Running);

    assert!(publisher.mark_cancelling("turn-b").is_some());
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
