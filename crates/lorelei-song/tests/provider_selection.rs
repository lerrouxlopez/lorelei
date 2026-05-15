use lorelei_core::Config;
use lorelei_song::build_song_provider;

#[test]
fn selects_provider_from_config() {
    let toml = r#"
        [providers.p]
        kind = "openai-compatible"
        base_url = "https://example.invalid/v1"
        model = "x"
        api_key = { source = "literal", value = "dont-use-real-keys" }

        [song]
        provider = { name = "p" }
    "#;

    let cfg = Config::from_toml_str(toml).unwrap();
    let _ = build_song_provider(&cfg).unwrap();
}

