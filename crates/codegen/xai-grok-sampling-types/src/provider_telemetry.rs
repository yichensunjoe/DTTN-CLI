//! Provider-neutral account and quota telemetry for status-line consumers.
//!
//! Provider adapters own authentication and HTTP behavior. This module only
//! defines redacted snapshots and safe display gates so unsupported providers do
//! not accidentally appear to have a zero balance.

use serde::{Deserialize, Serialize};

use crate::model_catalog::MetadataSource;

/// Whether provider account telemetry can currently be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountTelemetryState {
    /// A fresh provider response was parsed successfully.
    Available,
    /// The provider does not expose a supported balance or quota endpoint.
    Unsupported,
    /// Credentials are missing or cannot be used for this endpoint.
    AuthRequired,
    /// Credentials are valid but lack billing or usage permissions.
    PermissionDenied,
    /// A supported endpoint failed transiently.
    TemporarilyUnavailable,
    /// Organization policy disabled account telemetry.
    DisabledByPolicy,
}

/// Currency amount represented in millionths of the named currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoneyAmount {
    /// ISO-4217 code when available, for example `USD` or `CNY`.
    pub currency: String,
    pub micro_units: i64,
}

/// Unit used by a provider quota.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaUnit {
    Requests,
    Tokens,
    ProviderCredits,
    Custom(String),
}

/// Remaining quota for the active account or API key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub remaining: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    pub unit: QuotaUnit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_unix_ms: Option<u64>,
}

/// Rate-limit values extracted from response headers or provider APIs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests_remaining: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_remaining: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_unix_ms: Option<u64>,
}

/// Redacted provider account snapshot consumed by Doctor and the status line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAccountSnapshot {
    pub provider_id: String,
    pub state: AccountTelemetryState,
    pub source: MetadataSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<MoneyAmount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<RateLimitSnapshot>,
    pub fetched_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<u64>,
}

impl ProviderAccountSnapshot {
    pub fn is_stale(&self, now_unix_ms: u64) -> bool {
        self.expires_at_unix_ms
            .is_some_and(|expires_at| now_unix_ms >= expires_at)
    }

    /// Return a balance only when the provider explicitly reported one and the
    /// snapshot is fresh. Unsupported and failed endpoints never become `$0`.
    pub fn balance_for_display(&self, now_unix_ms: u64) -> Option<&MoneyAmount> {
        (self.state == AccountTelemetryState::Available && !self.is_stale(now_unix_ms))
            .then_some(self.balance.as_ref())
            .flatten()
    }

    pub fn quota_for_display(&self, now_unix_ms: u64) -> Option<&QuotaSnapshot> {
        (self.state == AccountTelemetryState::Available && !self.is_stale(now_unix_ms))
            .then_some(self.quota.as_ref())
            .flatten()
    }

    pub fn rate_limits_for_display(&self, now_unix_ms: u64) -> Option<&RateLimitSnapshot> {
        (self.state == AccountTelemetryState::Available && !self.is_stale(now_unix_ms))
            .then_some(self.rate_limits.as_ref())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(state: AccountTelemetryState) -> ProviderAccountSnapshot {
        ProviderAccountSnapshot {
            provider_id: "example".to_string(),
            state,
            source: MetadataSource::ProviderApi,
            account_label: None,
            balance: Some(MoneyAmount {
                currency: "USD".to_string(),
                micro_units: 10_000_000,
            }),
            quota: None,
            rate_limits: None,
            fetched_at_unix_ms: 1_000,
            expires_at_unix_ms: Some(2_000),
        }
    }

    #[test]
    fn fresh_available_balance_is_visible() {
        let snapshot = snapshot(AccountTelemetryState::Available);
        assert_eq!(
            snapshot.balance_for_display(1_500).unwrap().micro_units,
            10_000_000
        );
    }

    #[test]
    fn unsupported_provider_never_looks_like_zero_balance() {
        let snapshot = snapshot(AccountTelemetryState::Unsupported);
        assert!(snapshot.balance_for_display(1_500).is_none());
    }

    #[test]
    fn stale_balance_is_hidden() {
        let snapshot = snapshot(AccountTelemetryState::Available);
        assert!(snapshot.balance_for_display(2_000).is_none());
    }

    #[test]
    fn no_expiry_means_not_stale() {
        let mut snapshot = snapshot(AccountTelemetryState::Available);
        snapshot.expires_at_unix_ms = None;
        assert!(!snapshot.is_stale(u64::MAX));
    }
}
