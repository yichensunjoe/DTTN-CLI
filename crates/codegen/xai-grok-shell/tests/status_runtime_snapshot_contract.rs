use xai_grok_shell::session::status_runtime_snapshot::{
    StatusRunState, StatusRuntimePublisher, StatusRuntimeSnapshot, StatusTokenUsage,
    StatusUsageTotals,
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
fn usage_publication_uses_absolute_session_ledger_values() {
    let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
    let published = publisher.publish_usage(
        Some(StatusUsageTotals {
            input_tokens: 120,
            output_tokens: 30,
            cached_read_tokens: 40,
            model_calls: 2,
            api_duration_ms: 900,
            cost_usd_ticks: Some(50_000),
            cost_trusted: true,
        }),
        Some(StatusUsageTotals {
            input_tokens: 1_200,
            output_tokens: 300,
            cached_read_tokens: 400,
            model_calls: 8,
            api_duration_ms: 5_000,
            cost_usd_ticks: Some(500_000),
            cost_trusted: true,
        }),
    );

    assert_eq!(published.tokens.turn_input, 120);
    assert_eq!(published.tokens.turn_output, 30);
    assert_eq!(published.tokens.turn_cached_input, 40);
    assert_eq!(published.tokens.session_input, 1_200);
    assert_eq!(published.tokens.session_output, 300);
    assert_eq!(published.tokens.session_cached_input, 400);
    assert_eq!(published.cost.turn_microunits, Some(5));
    assert_eq!(published.cost.session_microunits, Some(50));
    assert_eq!(published.latency.turn_api_duration_ms, Some(900));
    assert_eq!(published.latency.session_api_duration_ms, Some(5_000));
}

#[test]
fn untrusted_cost_is_absent_instead_of_rendered_as_free() {
    let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
    let published = publisher.publish_usage(
        Some(StatusUsageTotals {
            input_tokens: 1,
            model_calls: 1,
            cost_usd_ticks: Some(100_000),
            cost_trusted: false,
            ..Default::default()
        }),
        None,
    );
    assert_eq!(published.tokens.turn_input, 1);
    assert_eq!(published.cost.turn_microunits, None);
    assert_eq!(published.cost.currency, None);
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

#[test]
fn sampler_latency_is_prompt_owned_and_replaces_last_request() {
    let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
    publisher.begin_turn("turn-a");
    let first = publisher
        .publish_request_latency("turn-a", 120, Some(35))
        .expect("active prompt accepts latency");
    assert_eq!(first.latency.last_request_ms, Some(120));
    assert_eq!(first.latency.time_to_first_token_ms, Some(35));

    let revision = first.revision;
    publisher.begin_turn("turn-b");
    assert!(
        publisher
            .publish_request_latency("turn-a", 999, Some(998))
            .is_none()
    );
    let current = publisher.snapshot();
    assert_eq!(current.active_prompt_id.as_deref(), Some("turn-b"));
    assert_eq!(current.latency.last_request_ms, None);
    assert!(current.revision > revision);

    let latest = publisher
        .publish_request_latency("turn-b", 80, None)
        .expect("current prompt accepts latency");
    assert_eq!(latest.latency.last_request_ms, Some(80));
    assert_eq!(latest.latency.time_to_first_token_ms, None);
}

#[test]
fn backend_tool_events_are_idempotent_and_request_scoped() {
    let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
    publisher.begin_turn("turn-a");
    assert!(
        publisher
            .begin_backend_tool("request-1", "call-1", "web_search")
            .is_some()
    );
    assert!(
        publisher
            .begin_backend_tool("request-1", "call-1", "web_search")
            .is_none()
    );
    assert!(
        publisher
            .begin_backend_tool("request-1", "call-2", "x_search")
            .is_some()
    );
    assert_eq!(publisher.snapshot().tools.active_count, 2);

    assert!(
        publisher
            .finish_backend_tool("request-1", "call-1")
            .is_some()
    );
    assert!(
        publisher
            .finish_backend_tool("request-1", "call-1")
            .is_none()
    );
    assert_eq!(publisher.snapshot().tools.active_count, 1);

    assert!(publisher.finish_backend_request("request-1").is_some());
    let done = publisher.snapshot();
    assert_eq!(done.tools.active_count, 0);
    assert_eq!(done.tools.last_tool_name.as_deref(), Some("x_search"));
    assert!(done.latency.last_tool_ms.is_some());
}

#[test]
fn stale_backend_tool_cannot_overwrite_new_turn() {
    let publisher = StatusRuntimePublisher::new(StatusRuntimeSnapshot::default());
    publisher.begin_turn("turn-a");
    publisher.begin_backend_tool("request-a", "call-a", "web_search");
    publisher.begin_turn("turn-b");
    let revision = publisher.snapshot().revision;

    assert!(publisher.finish_backend_request("request-a").is_none());
    let current = publisher.snapshot();
    assert_eq!(current.revision, revision);
    assert_eq!(current.active_prompt_id.as_deref(), Some("turn-b"));
    assert_eq!(current.tools.active_count, 0);
    assert_eq!(current.tools.last_tool_name, None);
}
