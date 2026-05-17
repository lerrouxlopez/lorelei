#![forbid(unsafe_code)]

use tracing_subscriber::EnvFilter;

fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

/// Initialize a `tracing-subscriber` formatter based on environment variables.
///
/// - `RUST_LOG`: standard `tracing_subscriber::EnvFilter`
/// - `LORELEI_LOG_JSON=true`: emit JSON logs
///
/// This is best-effort and safe to call multiple times.
pub fn init_tracing(service_name: &'static str) {
    let json = env_bool("LORELEI_LOG_JSON");
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let base = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);

    let res = if json {
        base.json().flatten_event(true).try_init()
    } else {
        base.try_init()
    };

    if res.is_ok() {
        tracing::info!(service = service_name, "tracing initialized");
    }
}

/// Check if prompt logging is enabled via LORELEI_LOG_PROMPTS env var.
pub fn should_log_prompts() -> bool {
    env_bool("LORELEI_LOG_PROMPTS")
}

/// Redact sensitive data from logs.
pub fn redact_secret(label: &str) -> String {
    format!("{label}=REDACTED")
}

/// Log provider call with structured fields (name, model, latency_ms, retry_count, token_usage).
#[macro_export]
macro_rules! log_provider_call {
    (
        provider = $provider:expr,
        model = $model:expr,
        latency_ms = $latency_ms:expr,
        retry_count = $retry_count:expr
        $(, token_usage = $token_usage:expr)?
    ) => {
        tracing::info!(
            provider = $provider,
            model = $model,
            latency_ms = $latency_ms,
            retry_count = $retry_count,
            $(token_usage = ?$token_usage,)?
            "provider_call"
        );
    };
}

/// Log echo retrieval with structured fields (query_count, candidate_count, hit_count, latency_ms).
#[macro_export]
macro_rules! log_echo_retrieval {
    (
        query_count = $query_count:expr,
        candidate_count = $candidate_count:expr,
        hit_count = $hit_count:expr,
        latency_ms = $latency_ms:expr
    ) => {
        tracing::info!(
            query_count = $query_count,
            candidate_count = $candidate_count,
            hit_count = $hit_count,
            latency_ms = $latency_ms,
            "echo_retrieval"
        );
    };
}

/// Log siren policy decision with structured fields (decision, risk_level, reason).
#[macro_export]
macro_rules! log_siren_decision {
    (
        decision = $decision:expr,
        risk_level = $risk_level:expr,
        reason = $reason:expr
    ) => {
        tracing::info!(
            decision = $decision,
            risk_level = $risk_level,
            reason = $reason,
            "siren_decision"
        );
    };
}

/// Log shell call with structured fields (shell_name, risk_level, status, latency_ms).
#[macro_export]
macro_rules! log_shell_call {
    (
        shell_name = $shell_name:expr,
        risk_level = $risk_level:expr,
        status = $status:expr,
        latency_ms = $latency_ms:expr
    ) => {
        tracing::info!(
            shell_name = $shell_name,
            risk_level = $risk_level,
            status = $status,
            latency_ms = $latency_ms,
            "shell_call"
        );
    };
}
