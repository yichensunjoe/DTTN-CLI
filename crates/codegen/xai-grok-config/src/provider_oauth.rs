//! OAuth 2.0 Device Flow & Token Management for LLM Providers (e.g. Kimi).

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};
use tracing::info;

const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const DEFAULT_KIMI_OAUTH_HOST: &str = "https://auth.kimi.com";
const KIMI_CLI_VERSION: &str = "0.14.0";
const OAUTH_EXPIRY_SKEW_MS: u64 = 5 * 60 * 1000; // 5 minutes skew buffer

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAuthResponse {
    pub user_code: String,
    pub device_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in_ms: u64,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredential {
    pub access: String,
    pub refresh: String,
    pub expires: u64, // Unix timestamp in milliseconds
    pub account_id: Option<String>,
    pub email: Option<String>,
}

impl OAuthCredential {
    pub fn is_expired(&self) -> bool {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.expires <= now_ms + OAUTH_EXPIRY_SKEW_MS
    }
}

#[derive(Deserialize)]
struct RawDeviceAuthResponse {
    user_code: Option<String>,
    device_code: Option<String>,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct RawTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

pub struct KimiOAuthClient;

impl KimiOAuthClient {
    fn oauth_host() -> String {
        std::env::var("KIMI_CODE_OAUTH_HOST")
            .or_else(|_| std::env::var("KIMI_OAUTH_HOST"))
            .unwrap_or_else(|_| DEFAULT_KIMI_OAUTH_HOST.to_string())
    }

    fn device_id() -> String {
        let home = crate::dttn_home();
        let path = home.join("kimi-device-id");
        if let Ok(content) = fs::read_to_string(&path) {
            let id = content.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }
        let new_id = uuid::Uuid::new_v4().simple().to_string();
        let _ = fs::create_dir_all(&home);
        let _ = fs::write(&path, format!("{}\n", new_id));
        new_id
    }

    fn common_headers() -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            format!("KimiCLI/{}", KIMI_CLI_VERSION).parse().unwrap(),
        );
        headers.insert("X-Msh-Platform", "kimi_code_cli".parse().unwrap());
        headers.insert("X-Msh-Version", KIMI_CLI_VERSION.parse().unwrap());
        headers.insert(
            "X-Msh-Device-Name",
            gethostname::gethostname().to_string_lossy().parse().unwrap(),
        );
        headers.insert(
            "X-Msh-Device-Model",
            format!("{} {}", std::env::consts::OS, std::env::consts::ARCH).parse().unwrap(),
        );
        headers.insert("X-Msh-Os-Version", std::env::consts::OS.parse().unwrap());
        headers.insert("X-Msh-Device-Id", Self::device_id().parse().unwrap());
        headers
    }

    pub async fn request_device_authorization() -> Result<DeviceAuthResponse, String> {
        let client = reqwest::Client::builder()
            .default_headers(Self::common_headers())
            .build()
            .map_err(|e| e.to_string())?;

        let url = format!("{}/api/oauth/device_authorization", Self::oauth_host());
        let res = client
            .post(&url)
            .form(&[("client_id", KIMI_CLIENT_ID)])
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(format!("Kimi device authorization failed: {}", text));
        }

        let raw: RawDeviceAuthResponse = res
            .json()
            .await
            .map_err(|e| format!("Failed to parse Kimi response: {}", e))?;

        let user_code = raw.user_code.ok_or("Missing user_code")?;
        let device_code = raw.device_code.ok_or("Missing device_code")?;
        let verification_uri = raw.verification_uri.ok_or("Missing verification_uri")?;
        let verification_uri_complete = raw
            .verification_uri_complete
            .unwrap_or_else(|| verification_uri.clone());

        let expires_in_ms = raw.expires_in.unwrap_or(900) * 1000;
        let interval_ms = raw.interval.unwrap_or(5).max(1) * 1000;

        Ok(DeviceAuthResponse {
            user_code,
            device_code,
            verification_uri,
            verification_uri_complete,
            expires_in_ms,
            interval_ms,
        })
    }

    pub async fn poll_for_token<F>(
        device_code: &str,
        interval_ms: u64,
        expires_in_ms: u64,
        on_poll: F,
    ) -> Result<OAuthCredential, String>
    where
        F: Fn(),
    {
        let client = reqwest::Client::builder()
            .default_headers(Self::common_headers())
            .build()
            .map_err(|e| e.to_string())?;

        let url = format!("{}/api/oauth/token", Self::oauth_host());
        let start = SystemTime::now();
        let mut poll_interval = Duration::from_millis(interval_ms);

        loop {
            let elapsed = start.elapsed().unwrap_or_default().as_millis() as u64;
            if elapsed >= expires_in_ms {
                return Err("Kimi device authorization flow timed out".to_string());
            }

            on_poll();

            let res = client
                .post(&url)
                .form(&[
                    ("client_id", KIMI_CLIENT_ID),
                    ("device_code", device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await;

            if let Ok(res) = res {
                if let Ok(raw) = res.json::<RawTokenResponse>().await {
                    if let (Some(access), Some(refresh)) = (raw.access_token, raw.refresh_token) {
                        let expires_in_sec = raw.expires_in.unwrap_or(86400);
                        let now_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let expires = now_ms + expires_in_sec * 1000 - OAUTH_EXPIRY_SKEW_MS;

                        return Ok(OAuthCredential {
                            access,
                            refresh,
                            expires,
                            account_id: None,
                            email: None,
                        });
                    }

                    if let Some(err) = raw.error {
                        match err.as_str() {
                            "authorization_pending" => {}
                            "slow_down" => {
                                poll_interval += Duration::from_secs(5);
                            }
                            "expired_token" => return Err("Kimi device authorization code expired".to_string()),
                            "access_denied" => return Err("Kimi device authorization access denied by user".to_string()),
                            other => return Err(format!("Kimi device flow error: {} - {}", other, raw.error_description.unwrap_or_default())),
                        }
                    }
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    pub async fn refresh_token(refresh_token: &str) -> Result<OAuthCredential, String> {
        let client = reqwest::Client::builder()
            .default_headers(Self::common_headers())
            .build()
            .map_err(|e| e.to_string())?;

        let url = format!("{}/api/oauth/token", Self::oauth_host());
        let res = client
            .post(&url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", KIMI_CLIENT_ID),
            ])
            .send()
            .await
            .map_err(|e| format!("Token refresh request failed: {}", e))?;

        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(format!("Kimi token refresh failed: {}", text));
        }

        let raw: RawTokenResponse = res
            .json()
            .await
            .map_err(|e| format!("Failed to parse token response: {}", e))?;

        let access = raw.access_token.ok_or("Missing access_token in refresh response")?;
        let refresh = raw.refresh_token.unwrap_or_else(|| refresh_token.to_string());
        let expires_in_sec = raw.expires_in.unwrap_or(86400);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Ok(OAuthCredential {
            access,
            refresh,
            expires: now_ms + expires_in_sec * 1000 - OAUTH_EXPIRY_SKEW_MS,
            account_id: None,
            email: None,
        })
    }
}

pub fn oauth_credential_path(provider: &str) -> PathBuf {
    let home = crate::dttn_home();
    home.join(format!("oauth_credentials_{}.json", provider))
}

pub fn save_oauth_credential(provider: &str, cred: &OAuthCredential) -> Result<(), String> {
    let path = oauth_credential_path(provider);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(cred).map_err(|e| e.to_string())?;
    fs::write(&path, data).map_err(|e| e.to_string())?;
    info!("Saved OAuth credentials for provider '{}' to {:?}", provider, path);
    Ok(())
}

pub fn load_oauth_credential(provider: &str) -> Option<OAuthCredential> {
    let path = oauth_credential_path(provider);
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub async fn get_or_refresh_valid_token(provider: &str) -> Result<String, String> {
    let mut cred = load_oauth_credential(provider)
        .ok_or_else(|| format!("No OAuth credential found for provider '{}'. Run 'dttn login --provider {}'", provider, provider))?;

    if cred.is_expired() {
        info!("OAuth token for provider '{}' is expired/expiring soon, refreshing...", provider);
        match provider {
            "kimi" | "moonshot" => {
                let fresh = KimiOAuthClient::refresh_token(&cred.refresh).await?;
                cred = fresh;
                save_oauth_credential(provider, &cred)?;
            }
            other => return Err(format!("Unsupported OAuth provider for auto-refresh: {}", other)),
        }
    }

    Ok(cred.access)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_credential_save_and_load() {
        let temp_dir = tempfile::tempdir().unwrap();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let cred = OAuthCredential {
            access: "test_access_token".to_string(),
            refresh: "test_refresh_token".to_string(),
            expires: now_ms + 3600 * 1000,
            account_id: None,
            email: None,
        };

        let file_path = temp_dir.path().join("test_kimi_oauth.json");
        let content = serde_json::to_string_pretty(&cred).unwrap();
        std::fs::write(&file_path, content).unwrap();

        let loaded_cred: OAuthCredential =
            serde_json::from_str(&std::fs::read_to_string(&file_path).unwrap()).unwrap();
        assert_eq!(loaded_cred.access, "test_access_token");
        assert_eq!(loaded_cred.refresh, "test_refresh_token");
        assert!(!loaded_cred.is_expired());
    }


    #[test]
    fn test_expired_credential() {
        let cred = OAuthCredential {
            access: "old_access".to_string(),
            refresh: "old_refresh".to_string(),
            expires: 0, // Very old timestamp -> expired
            account_id: None,
            email: None,
        };
        assert!(cred.is_expired());
    }
}


