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
        .with_target(false)
        .with_current_span(true)
        .with_span_list(true);

    let res = if json {
        base.json()
            .flatten_event(true)
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_thread_ids(false)
            .with_thread_names(false)
            .try_init()
    } else {
        base.with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .try_init()
    };

    if res.is_ok() {
        tracing::info!(service = service_name, "tracing initialized");
    }
}

