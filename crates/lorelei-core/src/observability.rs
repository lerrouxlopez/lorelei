fn env_flag_true(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "y" | "on")
        }
        Err(_) => false,
    }
}

/// Whether Lorelei is allowed to log full prompts.
///
/// Defaults to `false`. Enable with `LORELEI_LOG_PROMPTS=true`.
pub fn log_prompts_enabled() -> bool {
    env_flag_true("LORELEI_LOG_PROMPTS")
}

/// Whether logs should be emitted as structured JSON.
///
/// Defaults to `false`. Enable with `LORELEI_LOG_JSON=true`.
pub fn log_json_enabled() -> bool {
    env_flag_true("LORELEI_LOG_JSON")
}

