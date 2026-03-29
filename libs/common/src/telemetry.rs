//! OpenTelemetry telemetry initialization for Recast MCP services.
//!
//! Provides structured JSON logging with trace/span ID correlation, OTLP gRPC
//! trace export, and a [`TelemetryGuard`] that flushes pending spans on drop.

use opentelemetry::trace::TraceContextExt as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::{trace as sdktrace, Resource};
use std::env;
use std::fmt;
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{
    fmt::{format::Writer, FmtContext, FormatEvent, FormatFields},
    layer::SubscriberExt as _,
    registry::LookupSpan,
    util::SubscriberInitExt as _,
    EnvFilter,
};

// ─── Guard ───────────────────────────────────────────────────────────────────

/// Flushes all pending OTEL spans when dropped.
///
/// Must be stored in a variable that lives for the entire process lifetime.
/// Dropping this value will call [`opentelemetry::global::shutdown_tracer_provider`],
/// which blocks until all queued spans are exported.
pub struct TelemetryGuard;

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        global::shutdown_tracer_provider();
    }
}

// ─── Error ───────────────────────────────────────────────────────────────────

/// Error returned when telemetry initialization fails.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// The OTLP span exporter could not be constructed.
    #[error("OTLP exporter init failed: {0}")]
    OtlpInit(String),
    /// A global tracing subscriber was already installed.
    #[error("tracing subscriber already initialized")]
    AlreadyInitialized,
}

// ─── JSON formatter ───────────────────────────────────────────────────────────

/// Formats tracing events as structured JSON lines with OTEL trace correlation.
///
/// Emits one JSON object per log line with fields:
/// `timestamp`, `level`, `service`, `version`, `message`,
/// and optionally `trace_id` / `span_id` when inside an active OTEL span.
struct JsonEventFormatter {
    service: &'static str,
    version: &'static str,
}

impl<S, N> FormatEvent<S, N> for JsonEventFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        let timestamp = chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let level = event.metadata().level().as_str().to_lowercase();

        // Collect the log message from the event fields.
        let mut message = String::new();
        let mut visitor = MessageVisitor(&mut message);
        event.record(&mut visitor);

        // Read OTEL trace_id / span_id from the current span context.
        let span = tracing::Span::current();
        let otel_ctx = span.context();
        let otel_span_ref = otel_ctx.span();
        let span_ctx = otel_span_ref.span_context();

        let mut obj = serde_json::Map::new();
        obj.insert("timestamp".into(), serde_json::Value::String(timestamp));
        obj.insert(
            "level".into(),
            serde_json::Value::String(level),
        );
        obj.insert(
            "service".into(),
            serde_json::Value::String(self.service.to_string()),
        );
        obj.insert(
            "version".into(),
            serde_json::Value::String(self.version.to_string()),
        );
        obj.insert("message".into(), serde_json::Value::String(message));

        if span_ctx.is_valid() {
            obj.insert(
                "trace_id".into(),
                serde_json::Value::String(span_ctx.trace_id().to_string()),
            );
            obj.insert(
                "span_id".into(),
                serde_json::Value::String(span_ctx.span_id().to_string()),
            );
        }

        let json =
            serde_json::to_string(&serde_json::Value::Object(obj)).map_err(|_| fmt::Error)?;

        writeln!(writer, "{json}")
    }
}

// Minimal visitor that extracts the "message" field from a tracing event.
struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            *self.0 = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            *self.0 = format!("{value:?}");
        }
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Initialize structured JSON logging and OpenTelemetry distributed tracing.
///
/// Reads the following environment variables:
/// - `RUST_LOG` — log filter directives (default: `info`)
/// - `OTEL_EXPORTER_OTLP_ENDPOINT` — OTLP gRPC collector URL (default: `http://localhost:4317`)
/// - `OTEL_SDK_DISABLED` — set to `true` to disable OTEL export entirely (default: `false`)
///
/// Returns a [`TelemetryGuard`] that **must be held** for the process lifetime.
/// Dropping the guard flushes all pending spans before process exit.
///
/// # Errors
///
/// Returns [`TelemetryError`] if the OTLP exporter cannot be initialized or
/// if a global subscriber is already installed.
pub fn init_telemetry(
    service_name: &'static str,
    service_version: &'static str,
) -> Result<TelemetryGuard, TelemetryError> {
    let disabled = env::var("OTEL_SDK_DISABLED")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .event_format(JsonEventFormatter {
            service: service_name,
            version: service_version,
        })
        .with_writer(std::io::stdout);

    if disabled {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .try_init()
            .map_err(|_| TelemetryError::AlreadyInitialized)?;
        return Ok(TelemetryGuard);
    }

    let endpoint = env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:4317".to_string());

    let resource = Resource::new(vec![
        KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_NAME,
            service_name,
        ),
        KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
            service_version,
        ),
    ]);

    let provider = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .tonic()
                .with_endpoint(endpoint),
        )
        .with_trace_config(sdktrace::Config::default().with_resource(resource))
        .install_batch(opentelemetry_sdk::runtime::Tokio)
        .map_err(|e| TelemetryError::OtlpInit(e.to_string()))?;

    let tracer = provider.tracer(service_name);
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init()
        .map_err(|_| TelemetryError::AlreadyInitialized)?;

    Ok(TelemetryGuard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_guard_implements_drop() {
        // Verify TelemetryGuard is constructable and droppable (does not panic).
        // OTEL_SDK_DISABLED=true so no real exporter is wired.
        // We can't init the global subscriber twice, so just test the struct.
        let _guard = TelemetryGuard;
        // Drop happens here. global::shutdown_tracer_provider is called,
        // which is a no-op when no provider has been installed.
    }
}
