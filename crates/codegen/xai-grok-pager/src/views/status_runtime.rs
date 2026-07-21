//! Pure, I/O-free rendering helpers for the status-runtime wire snapshot.
//!
//! This module only formats an already-prepared in-memory DTO. It must never
//! fetch session data, inspect configuration, query Git, or perform async work.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use xai_grok_shell::session::status_runtime_snapshot::{StatusRunState, StatusRuntimeWireSnapshot};

use crate::theme::Theme;

const SEPARATOR: &str = " | ";

/// Cached plain text for one immutable status generation and render geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusRuntimeRenderCache {
    revision: u64,
    context_used: Option<u64>,
    context_total: Option<u64>,
    available_width: u16,
    text: String,
}

/// Return whether an incoming immutable generation supersedes the current one.
#[doc(hidden)]
pub fn should_apply_revision(current_revision: Option<u64>, incoming_revision: u64) -> bool {
    current_revision.is_none_or(|current| incoming_revision > current)
}

/// Build a styled status line while avoiding repeated string construction when
/// the snapshot revision, context counters, and available width are unchanged.
pub(crate) fn status_runtime_line(
    cache: &mut Option<StatusRuntimeRenderCache>,
    snapshot: &StatusRuntimeWireSnapshot,
    context_used: Option<u64>,
    context_total: Option<u64>,
    available_width: u16,
    theme: &Theme,
) -> Line<'static> {
    let cache_matches = cache.as_ref().is_some_and(|cached| {
        cached.revision == snapshot.revision
            && cached.context_used == context_used
            && cached.context_total == context_total
            && cached.available_width == available_width
    });

    if !cache_matches {
        *cache = Some(StatusRuntimeRenderCache {
            revision: snapshot.revision,
            context_used,
            context_total,
            available_width,
            text: status_runtime_text(snapshot, context_used, context_total, available_width),
        });
    }

    let text = cache
        .as_ref()
        .map(|cached| cached.text.clone())
        .unwrap_or_default();
    Line::from(Span::styled(
        text,
        Style::default().fg(theme.text_primary).bg(theme.bg_base),
    ))
}

/// Select the richest representation that fits the supplied terminal width.
///
/// Public for the dedicated integration contract test; production callers should
/// normally use [`status_runtime_line`].
#[doc(hidden)]
pub fn status_runtime_text(
    snapshot: &StatusRuntimeWireSnapshot,
    context_used: Option<u64>,
    context_total: Option<u64>,
    available_width: u16,
) -> String {
    let max_width = usize::from(available_width);
    if max_width == 0 {
        return String::new();
    }

    let model = sanitize_label(&snapshot.model_id, "unknown");
    let context_percent = context_percent(context_used, context_total);
    let cost = trusted_cost(snapshot);
    let tool = tool_label(snapshot);
    let pending = (snapshot.pending_interactions > 0)
        .then(|| format!("input {}", snapshot.pending_interactions));

    let mut wide = vec![
        model.clone(),
        run_state_label(snapshot.run_state).to_owned(),
    ];
    if let Some(percent) = context_percent {
        wide.push(format!("ctx {percent}%"));
    }
    wide.push(format!(
        "{} tok",
        format_tokens_compact(
            snapshot
                .session_input_tokens
                .saturating_add(snapshot.session_output_tokens)
        )
    ));
    if let Some(cost) = cost.clone() {
        wide.push(cost);
    }
    if let Some(latency) = latency_label(snapshot) {
        wide.push(latency);
    }
    if let Some(tool) = tool.clone() {
        wide.push(tool);
    }
    if let Some(pending) = pending.clone() {
        wide.push(pending);
    }
    let wide = wide.join(SEPARATOR);
    if wide.width() <= max_width {
        return wide;
    }

    let mut medium = vec![model.clone()];
    if let Some(percent) = context_percent {
        medium.push(format!("ctx {percent}%"));
    } else {
        medium.push(run_state_label(snapshot.run_state).to_owned());
    }
    if let Some(cost) = cost {
        medium.push(cost);
    }
    if let Some(tool) = tool {
        medium.push(tool);
    }
    if let Some(pending) = pending {
        medium.push(pending);
    }
    let medium = medium.join(SEPARATOR);
    if medium.width() <= max_width {
        return medium;
    }

    let suffix = context_percent
        .map(|percent| format!("{SEPARATOR}{percent}%"))
        .unwrap_or_else(|| format!("{SEPARATOR}{}", run_state_label(snapshot.run_state)));
    let suffix_width = suffix.width();
    if suffix_width < max_width {
        let model_width = max_width - suffix_width;
        return format!("{}{}", truncate_to_width(&model, model_width), suffix);
    }

    truncate_to_width(&model, max_width)
}

fn context_percent(used: Option<u64>, total: Option<u64>) -> Option<u8> {
    let used = used?;
    let total = total.filter(|total| *total > 0)?;
    Some(
        used.saturating_mul(100)
            .checked_div(total)
            .unwrap_or(100)
            .min(100) as u8,
    )
}

fn run_state_label(state: StatusRunState) -> &'static str {
    match state {
        StatusRunState::Idle => "idle",
        StatusRunState::Running => "running",
        StatusRunState::WaitingForInput => "waiting",
        StatusRunState::Cancelling => "cancelling",
        StatusRunState::Failed => "failed",
    }
}

fn trusted_cost(snapshot: &StatusRuntimeWireSnapshot) -> Option<String> {
    let currency = snapshot.cost_currency.as_deref()?;
    if !currency.eq_ignore_ascii_case("USD") {
        return None;
    }
    snapshot.session_cost_microunits.map(format_usd_microunits)
}

fn latency_label(snapshot: &StatusRuntimeWireSnapshot) -> Option<String> {
    if let Some(ms) = snapshot.time_to_first_token_ms {
        return Some(format!("ttft {}", format_duration_ms(ms)));
    }
    snapshot
        .last_request_ms
        .map(|ms| format!("req {}", format_duration_ms(ms)))
}

fn tool_label(snapshot: &StatusRuntimeWireSnapshot) -> Option<String> {
    let total = snapshot.active_tools.saturating_add(snapshot.queued_tools);
    if total > 0 {
        return Some(format!("tools {total}"));
    }

    let name = snapshot.last_tool_name.as_deref()?;
    let name = truncate_to_width(&sanitize_label(name, "tool"), 16);
    snapshot
        .last_tool_ms
        .map(|ms| format!("tool {name} {}", format_duration_ms(ms)))
        .or_else(|| Some(format!("tool {name}")))
}

fn format_tokens_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format_decimal_unit(tokens, 1_000_000, 100_000, "M")
    } else if tokens >= 1_000 {
        format_decimal_unit(tokens, 1_000, 100, "k")
    } else {
        tokens.to_string()
    }
}

fn format_decimal_unit(value: u64, unit: u64, tenth: u64, suffix: &str) -> String {
    let whole = value / unit;
    let decimal = (value % unit) / tenth;
    if decimal == 0 {
        format!("{whole}{suffix}")
    } else {
        format!("{whole}.{decimal}{suffix}")
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        let whole = ms / 1_000;
        let tenth = (ms % 1_000) / 100;
        return if tenth == 0 {
            format!("{whole}s")
        } else {
            format!("{whole}.{tenth}s")
        };
    }
    let minutes = ms / 60_000;
    let seconds = (ms % 60_000) / 1_000;
    if seconds == 0 {
        format!("{minutes}m")
    } else {
        format!("{minutes}m{seconds}s")
    }
}

fn format_usd_microunits(microunits: u64) -> String {
    let whole = microunits / 1_000_000;
    let fractional = microunits % 1_000_000;
    if fractional == 0 {
        return format!("${whole}");
    }

    let mut fractional = format!("{fractional:06}");
    while fractional.ends_with('0') {
        fractional.pop();
    }
    format!("${whole}.{fractional}")
}

fn sanitize_label(value: &str, fallback: &str) -> String {
    let sanitized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.is_empty() {
        fallback.to_owned()
    } else {
        sanitized
    }
}

fn truncate_to_width(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if value.width() <= max_width {
        return value.to_owned();
    }
    if max_width == 1 {
        return "…".to_owned();
    }

    let content_width = max_width - 1;
    let mut width = 0;
    let mut output = String::new();
    for grapheme in value.graphemes(true) {
        let grapheme_width = grapheme.width();
        if width + grapheme_width > content_width {
            break;
        }
        output.push_str(grapheme);
        width += grapheme_width;
    }
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn wide_status_contains_complete_runtime_summary() {
        let text = status_runtime_text(&snapshot(), Some(84_000), Some(200_000), 160);
        assert_eq!(
            text,
            "openai/gpt-5 | running | ctx 42% | 128k tok | $0.0421 | ttft 680ms | tools 2 | input 1"
        );
    }

    #[test]
    fn medium_status_drops_secondary_latency_and_token_details() {
        let text = status_runtime_text(&snapshot(), Some(84_000), Some(200_000), 55);
        assert_eq!(text, "openai/gpt-5 | ctx 42% | $0.0421 | tools 2 | input 1");
        assert!(!text.contains("ttft"));
        assert!(!text.contains(" tok"));
    }

    #[test]
    fn narrow_status_keeps_model_and_context_percentage() {
        let text = status_runtime_text(&snapshot(), Some(84_000), Some(200_000), 20);
        assert_eq!(text, "openai/gpt-5 | 42%");
    }

    #[test]
    fn unknown_or_non_usd_cost_is_not_rendered_as_zero() {
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
    fn unicode_and_long_model_names_are_width_bounded() {
        let mut status = snapshot();
        status.model_id = "公司内部/超长模型-推理版本-🚀-alpha".into();
        let text = status_runtime_text(&status, Some(84_000), Some(200_000), 18);
        assert!(text.width() <= 18);
        assert!(text.contains("42%"));
    }

    #[test]
    fn completed_tool_and_pending_interaction_are_visible() {
        let mut status = snapshot();
        status.active_tools = 0;
        status.pending_interactions = 2;
        status.last_tool_name = Some("company_search".into());
        status.last_tool_ms = Some(1_250);
        let text = status_runtime_text(&status, Some(1), Some(2), 200);
        assert!(text.contains("tool company_search 1.2s"));
        assert!(text.contains("input 2"));
    }

    #[test]
    fn renderer_module_has_no_io_or_rpc_dependencies() {
        let source = include_str!("status_runtime.rs");
        for forbidden in [
            concat!("std", "::fs"),
            concat!("tokio", "::fs"),
            concat!("req", "west"),
            concat!("Command", "::"),
            concat!("git", "2::"),
            concat!("session", "/info"),
            concat!("block", "_on"),
        ] {
            assert!(
                !source.contains(forbidden),
                "status runtime renderer must not depend on {forbidden}"
            );
        }
    }
}
