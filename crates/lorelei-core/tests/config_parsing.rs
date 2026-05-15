use lorelei_core::{ApiKeySource, Config, ProviderConfig};

#[test]
fn parse_openai_compatible_env_key() {
    let toml = r#"
        [providers.p]
        kind = "openai-compatible"
        base_url = "https://example.invalid/v1"
        model = "x"
        api_key = { source = "env", var = "OPENAI_API_KEY" }

        [song]
        provider = { name = "p" }
    "#;

    let cfg = Config::from_toml_str(toml).unwrap();
    let provider = cfg.providers.get("p").unwrap();
    assert!(matches!(provider, ProviderConfig::OpenAiCompatible(_)));
}

#[test]
fn parse_anthropic_literal_key_redacts_debug() {
    let toml = r#"
        [providers.a]
        kind = "anthropic"
        model = "claude"
        api_key = { source = "literal", value = "super-secret" }
    "#;

    let cfg = Config::from_toml_str(toml).unwrap();
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("super-secret"));
    assert!(dbg.contains("REDACTED"));
}

#[test]
fn validation_fails_on_missing_song_provider_ref() {
    let toml = r#"
        [providers.p]
        kind = "local"
        endpoint = "http://127.0.0.1:1"
        model = "x"

        [song]
        provider = { name = "missing" }
    "#;

    assert!(Config::from_toml_str(toml).is_err());
}

#[test]
fn api_key_env_resolve_requires_var_present() {
    let key = ApiKeySource::Env {
        var: "LORELEI_TEST_MISSING_KEY".to_string(),
    };
    assert!(key.resolve().is_err());
}

