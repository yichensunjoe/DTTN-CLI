use unicode_width::UnicodeWidthStr;
use xai_grok_pager::views::status_runtime::{should_apply_revision, status_runtime_text};
use xai_grok_shell::session::status_runtime_snapshot::{StatusRunState, StatusRuntimeWireSnapshot};

fn snapshot() -> StatusRuntimeWireSnapshot {
    StatusRuntimeWireSnapshot {
        revision: 7,
        run_state: StatusRunState::Running,
        model_id: "openai/gpt-5".into(),
        context_window: 200_000,
        session_input_tokens: 120_000,
        session_output_tokens: 8_000,
        cost_currency: Some("USD".into()),
        session_cost_microunits: Some(42_100),
        time_to_first_token_ms: Some(680),
        active_tools: 2,
        pending_interactions: 1,
        ..Default::default()
    }
}

#[test]
fn wide_medium_and_narrow_layouts_degrade_deterministically() {
    let status = snapshot();

    let wide = status_runtime_text(&status, Some(84_000), Some(200_000), 160);
    assert_eq!(
        wide,
        "openai/gpt-5 | running | ctx 42% | 128k tok | $0.0421 | ttft 680ms | tools 2 | input 1"
    );

    let medium = status_runtime_text(&status, Some(84_000), Some(200_000), 55);
    assert_eq!(
        medium,
        "openai/gpt-5 | ctx 42% | $0.0421 | tools 2 | input 1"
    );
    assert!(!medium.contains("ttft"));
    assert!(!medium.contains(" tok"));

    let narrow = status_runtime_text(&status, Some(84_000), Some(200_000), 20);
    assert_eq!(narrow, "openai/gpt-5 | 42%");
}

#[test]
fn unknown_or_untrusted_cost_is_omitted_instead_of_rendered_as_zero() {
    let mut unknown = snapshot();
    unknown.session_cost_microunits = None;
    let text = status_runtime_text(&unknown, Some(1), Some(2), 160);
    assert!(!text.contains('$'));
    assert!(!text.contains("$0"));

    let mut non_usd = snapshot();
    non_usd.cost_currency = Some("EUR".into());
    let text = status_runtime_text(&non_usd, Some(1), Some(2), 160);
    assert!(!text.contains('$'));
}

#[test]
fn unicode_model_names_are_grapheme_safe_and_width_bounded() {
    let mut status = snapshot();
    status.model_id = "公司内部/超长模型-推理版本-🚀-alpha".into();
    let text = status_runtime_text(&status, Some(84_000), Some(200_000), 18);
    assert!(
        text.width() <= 18,
        "rendered {text:?} at width {}",
        text.width()
    );
    assert!(text.contains("42%"));
}

#[test]
fn tools_latency_and_pending_interactions_are_visible_when_space_allows() {
    let mut status = snapshot();
    status.active_tools = 0;
    status.queued_tools = 0;
    status.pending_interactions = 2;
    status.last_tool_name = Some("company_search".into());
    status.last_tool_ms = Some(1_250);
    let text = status_runtime_text(&status, Some(1), Some(2), 200);
    assert!(text.contains("tool company_search 1.2s"));
    assert!(text.contains("input 2"));
    assert!(text.contains("ttft 680ms"));
}

#[test]
fn revisions_accept_initial_and_newer_generations_only() {
    assert!(should_apply_revision(None, 0));
    assert!(should_apply_revision(Some(7), 8));
    assert!(!should_apply_revision(Some(7), 7));
    assert!(!should_apply_revision(Some(7), 6));
}

#[test]
fn renderer_and_notification_wiring_contract_remains_io_free_and_revision_gated() {
    let renderer = include_str!("../src/views/status_runtime.rs");
    for forbidden in [
        ["std", "::fs"].concat(),
        ["tokio", "::fs"].concat(),
        ["req", "west"].concat(),
        ["Command", "::"].concat(),
        ["git", "2::"].concat(),
        ["session", "/info"].concat(),
        ["block", "_on"].concat(),
    ] {
        assert!(
            !renderer.contains(&forbidden),
            "status runtime renderer must not depend on {forbidden}"
        );
    }

    let handler = include_str!("../src/app/acp_handler/mod.rs");
    for required in [
        "x.ai/status_runtime",
        "find_session_match",
        "SessionMatch::Root",
        "SessionMatch::Child",
        "apply_status_runtime",
    ] {
        assert!(
            handler.contains(required),
            "missing notification contract {required}"
        );
    }

    let session = include_str!("../src/app/agent_view/session.rs");
    assert!(session.contains("should_apply_revision"));
}
