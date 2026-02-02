use std::time::Duration;
use tracing::{Level, Span, field};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt, registry::LookupSpan};

#[derive(Clone, Debug)]
pub struct TraceId(String);

impl TraceId {
    pub fn new(v: impl Into<String>) -> Self {
        Self(v.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn init_tracing(json: bool) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let base = fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true)
        .with_file(true)
        // Includes timing when the span closes
        .with_span_events(fmt::format::FmtSpan::CLOSE);

    if json {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(base.json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(base.pretty())
            .init();
    }
}

pub fn root_span(name: &'static str, trace_id: &TraceId) -> Span {
    tracing::info_span!(
        "root",
        name = %name, 
        trace_id = %trace_id.as_str(),
        pair_id = field::Empty,
        session_id = field::Empty
    )
}

pub fn child_span(name: &'static str) -> Span {
    tracing::info_span!(
        "child",
        name = %name,
        pair_id = field::Empty,
        session_id = field::Empty
    )
}

pub fn annotate_span(pair_id: &str, session_id: Option<&uuid::Uuid>) {
    let span = Span::current();
    span.record("pair_id", &field::display(pair_id));
    if let Some(sid) = session_id {
        span.record("session_id", &field::display(sid));
    }
}

pub async fn warn_if_slow<F, T>(label: &'static str, max: Duration, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let start = std::time::Instant::now();
    let out = fut.await;
    let elapsed = start.elapsed();
    if elapsed > max {
        tracing::warn!(
            target: "performance",
            label = label,
            elapsed_ms = elapsed.as_millis() as u64,
            "slow operation detected"
        );
    }
    out
}
