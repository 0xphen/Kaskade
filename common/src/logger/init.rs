use once_cell::sync::OnceCell;
use tracing_subscriber::{fmt, EnvFilter};

static LOGGER_INIT: OnceCell<()> = OnceCell::new();

pub fn init_logger(service_name: &'static str) {
    LOGGER_INIT.get_or_init(|| {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info"));

        fmt()
            .with_env_filter(filter)
            .with_target(true) // <-- shows crate/module path
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_line_number(true)
            .with_span_events(fmt::format::FmtSpan::CLOSE)
            .init();

        tracing::info!(service = service_name, "logger initialized");
    });
}
