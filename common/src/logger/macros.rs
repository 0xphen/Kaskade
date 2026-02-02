use tracing::{Span, Level};
use super::TraceId;

/// Create a root span for a request / batch / job
pub fn root_span(name: &'static str, trace_id: &TraceId) -> Span {
    tracing::span!(
        Level::INFO,
        name,
        trace_id = %trace_id.as_str()
    )
}

/// Create a child span (inherits trace_id automatically)
pub fn child_span(name: &'static str) -> Span {
    tracing::span!(Level::INFO, name)
}
