use xai_grok_shell::agent::config::{
    ASSET_SERVER_URL_DEFAULT, CLI_CHAT_PROXY_BASE_URL_DEFAULT, DEFAULT_AGENT_TYPE,
    XAI_API_BASE_URL_DEFAULT, default_agent_type,
};

#[test]
fn default_agent_type_is_dttn_code_agent() {
    assert_eq!(DEFAULT_AGENT_TYPE, "dttn-code-agent");
    assert_eq!(default_agent_type(), "dttn-code-agent");
}

#[test]
fn built_in_service_defaults_are_fail_closed() {
    for endpoint in [
        CLI_CHAT_PROXY_BASE_URL_DEFAULT,
        XAI_API_BASE_URL_DEFAULT,
        ASSET_SERVER_URL_DEFAULT,
    ] {
        assert!(
            endpoint.contains(".dttn.invalid"),
            "built-in service endpoint must use the reserved .invalid namespace: {endpoint}",
        );
        assert!(
            !endpoint.contains("grok.com") && !endpoint.contains("x.ai"),
            "built-in service endpoint must not target an upstream vendor: {endpoint}",
        );
    }
}
