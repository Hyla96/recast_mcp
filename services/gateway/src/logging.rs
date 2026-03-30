//! Structured request/response logging for the MCP gateway.
//!
//! Writes one NDJSON record per `tools/call` and `tools/list` to stdout.
//! A tokio-buffered channel (capacity 4,096) decouples the hot path from I/O.
//! If the channel is full the record is silently dropped and the Prometheus
//! counter `gateway_log_drops_total` is incremented.
//!
//! # Usage
//!
//! ```ignore
//! let logger = RequestLogger::new(instance_id, LogLevel::Info);
//! let mut record = logger.new_record(server_id, "my-api", "tools/call", 42);
//! record.tool_name = Some("get_weather".to_string());
//! logger.log(record);
//! ```

use chrono::Utc;
use metrics::counter;
use opentelemetry::trace::TraceContextExt;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use url::Url;
use uuid::Uuid;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Channel buffer capacity; records beyond this are dropped.
const CHANNEL_CAPACITY: usize = 4_096;

/// Query parameter names that are stripped from logged URLs (case-insensitive).
const SENSITIVE_PARAMS: &[&str] = &["api_key", "token", "secret", "password", "key"];

// ── LogLevel ──────────────────────────────────────────────────────────────────

/// Logging verbosity level for the request logger.
///
/// Controls whether `initialize` / `ping` records are emitted (debug-only).
/// Parsed from the `LOG_LEVEL` environment variable; defaults to `Info`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// All records including internal trace records.
    Trace = 0,
    /// Debug records including `initialize` and `ping`.
    Debug = 1,
    /// Standard operational records (`tools/list`, `tools/call`).
    #[default]
    Info = 2,
    /// Only warnings and errors.
    Warn = 3,
    /// Only error-level records.
    Error = 4,
}

impl LogLevel {
    /// Parse a log level from an environment-variable string.
    ///
    /// Unrecognised values fall back to `Info`.
    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "info" => Self::Info,
            "warn" | "warning" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

// ── LogRecord ─────────────────────────────────────────────────────────────────

/// A single structured log record serialised as one NDJSON line.
///
/// Optional fields are skipped when absent so the NDJSON output stays compact.
#[derive(Debug, Serialize)]
pub struct LogRecord {
    /// RFC 3339 timestamp with millisecond precision (`2026-03-30T12:00:00.000Z`).
    pub timestamp: String,
    /// Unique ID of the target MCP server.
    pub server_id: Uuid,
    /// Human-readable server slug from the request URL.
    pub server_slug: String,
    /// JSON-RPC method (`tools/call`, `tools/list`, `initialize`, `ping`).
    pub method: String,
    /// Tool name for `tools/call`; absent for other methods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Upstream URL with sensitive query params redacted; absent for methods
    /// that do not make upstream calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_url: Option<String>,
    /// HTTP status code returned by the upstream; absent when no upstream call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<u16>,
    /// Total request latency in milliseconds (gateway perspective).
    pub latency_ms: u64,
    /// Time spent waiting for the upstream response in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_latency_ms: Option<u64>,
    /// Upstream response body size in bytes before transformation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_size_bytes: Option<usize>,
    /// Warnings emitted by the transform pipeline (empty when no warnings).
    pub transform_warnings: Vec<String>,
    /// First 8 characters of the bearer token (safe to log; no credential value).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_prefix: Option<String>,
    /// UUIDv4 identifying this gateway instance, generated at startup.
    pub instance_id: String,
    /// OpenTelemetry trace ID (32-char hex) for cross-service correlation.
    /// Absent when no active tracing span is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

// ── LogMsg ────────────────────────────────────────────────────────────────────

/// Internal channel message sent to the background log writer.
///
/// `Flush` is sent during graceful shutdown; the writer replies on the
/// provided oneshot once all prior `Record` messages have been written.
enum LogMsg {
    /// A structured log record to be serialised and written to stdout.
    ///
    /// Boxed to reduce enum variant size (clippy::large_enum_variant).
    Record(Box<LogRecord>),
    /// Sentinel sent during shutdown. The writer replies on `done` once all
    /// previously enqueued records have been processed.
    Flush(oneshot::Sender<()>),
}

// ── RequestLogger ─────────────────────────────────────────────────────────────

/// Asynchronous request logger backed by a bounded `tokio::sync::mpsc` channel.
///
/// Records are serialised to NDJSON and written to `stdout` by a dedicated
/// background task. [`RequestLogger::log`] is non-blocking and never panics.
pub struct RequestLogger {
    tx: mpsc::Sender<LogMsg>,
    /// UUIDv4 generated at gateway startup; included on every log record.
    pub instance_id: String,
    /// Minimum verbosity level to emit.
    level: LogLevel,
}

impl RequestLogger {
    /// Create a new logger and spawn the background writer task.
    ///
    /// The channel has a fixed capacity of 4,096 records. The spawned writer
    /// runs until all senders are dropped.
    pub fn new(instance_id: String, level: LogLevel) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<LogMsg>(CHANNEL_CAPACITY);
        tokio::spawn(run_writer(rx));
        Arc::new(Self { tx, instance_id, level })
    }

    /// Submit a record for writing.
    ///
    /// If the channel is full, the record is dropped and
    /// `gateway_log_drops_total` is incremented. This method never blocks.
    pub fn log(&self, record: LogRecord) {
        match self.tx.try_send(LogMsg::Record(Box::new(record))) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                counter!("gateway_log_drops_total").increment(1);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Background task has exited; nothing we can do.
                tracing::warn!("request log writer channel is closed; dropping record");
            }
        }
    }

    /// Flush all pending log records to stdout.
    ///
    /// Sends a [`LogMsg::Flush`] sentinel through the channel and waits for
    /// the background writer to acknowledge it. When this future resolves,
    /// all records enqueued before the call have been written to stdout.
    ///
    /// Used during graceful shutdown (Phase D/E) to ensure no records are
    /// silently dropped when the process exits.
    pub async fn flush(&self) {
        let (done_tx, done_rx) = oneshot::channel::<()>();
        // Use the async send (not try_send) at shutdown time so we wait for
        // any backpressure to clear rather than dropping the sentinel.
        match self.tx.send(LogMsg::Flush(done_tx)).await {
            Ok(()) => {
                // Wait for the writer to process all records up to the sentinel.
                let _ = done_rx.await;
            }
            Err(_) => {
                // Writer has already exited; nothing left to flush.
            }
        }
    }

    /// Whether the effective log level includes `Debug` (or finer).
    ///
    /// The router uses this to decide whether to emit `initialize` / `ping`
    /// records.
    #[must_use]
    pub fn is_debug_enabled(&self) -> bool {
        self.level <= LogLevel::Debug
    }

    /// Build a new record with common fields pre-populated.
    ///
    /// The caller fills in method-specific optional fields (`tool_name`,
    /// `upstream_url`, etc.) before passing the record to [`RequestLogger::log`].
    #[must_use]
    pub fn new_record(
        &self,
        server_id: Uuid,
        server_slug: &str,
        method: &str,
        latency_ms: u64,
    ) -> LogRecord {
        LogRecord {
            timestamp: Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            server_id,
            server_slug: server_slug.to_string(),
            method: method.to_string(),
            tool_name: None,
            upstream_url: None,
            upstream_status: None,
            latency_ms,
            upstream_latency_ms: None,
            response_size_bytes: None,
            transform_warnings: vec![],
            token_prefix: None,
            instance_id: self.instance_id.clone(),
            trace_id: current_trace_id(),
        }
    }
}

// ── sanitise_url ──────────────────────────────────────────────────────────────

/// Strip sensitive query parameters from a URL, returning a clean string.
///
/// Removes any query parameter whose name (case-insensitive) matches one of:
/// `api_key`, `token`, `secret`, `password`, `key`.
///
/// Non-sensitive parameters and the URL fragment are preserved unchanged.
#[must_use]
pub fn sanitise_url(url: &Url) -> String {
    let has_sensitive = url
        .query_pairs()
        .any(|(k, _)| is_sensitive_param(&k));

    if !has_sensitive {
        return url.to_string();
    }

    let kept: Vec<String> = url
        .query_pairs()
        .filter(|(k, _)| !is_sensitive_param(k))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    let mut sanitised = url.clone();
    if kept.is_empty() {
        sanitised.set_query(None);
    } else {
        sanitised.set_query(Some(&kept.join("&")));
    }
    sanitised.to_string()
}

fn is_sensitive_param(name: &str) -> bool {
    let lower = name.to_lowercase();
    SENSITIVE_PARAMS.iter().any(|&s| lower == s)
}

// ── current_trace_id ─────────────────────────────────────────────────────────

/// Extract the current OpenTelemetry trace ID as a 32-character hex string.
///
/// Returns `None` when no active span is present or telemetry is disabled.
#[must_use]
pub fn current_trace_id() -> Option<String> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let span_ctx = span.span_context();
    if span_ctx.is_valid() {
        Some(span_ctx.trace_id().to_string())
    } else {
        None
    }
}

// ── Background writer ─────────────────────────────────────────────────────────

/// Drain the log channel and write each record as one NDJSON line to stdout.
///
/// Handles two message variants:
/// - [`LogMsg::Record`]: serialise and print as NDJSON.
/// - [`LogMsg::Flush`]: send on the oneshot to signal that all prior records
///   have been written, then continue (remaining messages if any).
async fn run_writer(mut rx: mpsc::Receiver<LogMsg>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            LogMsg::Record(record) => {
                match serde_json::to_string(&record) {
                    Ok(line) => println!("{line}"),
                    Err(e) => {
                        // Serialisation should be infallible for this struct.
                        tracing::error!(error = %e, "failed to serialize request log record");
                    }
                }
            }
            LogMsg::Flush(done_tx) => {
                // All records enqueued before this sentinel have been processed.
                // Notify the caller and keep running in case more records arrive
                // (shouldn't happen at shutdown, but be robust).
                let _ = done_tx.send(());
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        missing_docs
    )]

    use super::*;
    use uuid::Uuid;

    fn make_url(s: &str) -> Url {
        Url::parse(s).expect("valid URL in test fixture")
    }

    // ── sanitise_url tests ────────────────────────────────────────────────────

    #[test]
    fn sanitise_url_keeps_non_sensitive_params() {
        let url = make_url("https://api.example.com/v1/weather?city=London&units=metric");
        let sanitised = sanitise_url(&url);
        assert!(sanitised.contains("city=London"), "{sanitised}");
        assert!(sanitised.contains("units=metric"), "{sanitised}");
    }

    #[test]
    fn sanitise_url_removes_api_key() {
        let url = make_url("https://api.example.com/v1/data?api_key=SECRET&page=1");
        let sanitised = sanitise_url(&url);
        assert!(!sanitised.contains("api_key"), "api_key must be removed: {sanitised}");
        assert!(!sanitised.contains("SECRET"), "value must be removed: {sanitised}");
        assert!(sanitised.contains("page=1"), "page param must be kept: {sanitised}");
    }

    #[test]
    fn sanitise_url_removes_token() {
        let url = make_url("https://api.example.com/data?token=abc123&format=json");
        let sanitised = sanitise_url(&url);
        assert!(!sanitised.contains("token="), "token must be removed: {sanitised}");
        assert!(sanitised.contains("format=json"), "{sanitised}");
    }

    #[test]
    fn sanitise_url_removes_secret() {
        let url = make_url("https://api.example.com/data?secret=xyz&ok=1");
        let sanitised = sanitise_url(&url);
        assert!(!sanitised.contains("secret"), "secret must be removed: {sanitised}");
        assert!(sanitised.contains("ok=1"), "{sanitised}");
    }

    #[test]
    fn sanitise_url_removes_password() {
        let url = make_url("https://api.example.com/data?password=hunter2&user=alice");
        let sanitised = sanitise_url(&url);
        assert!(!sanitised.contains("password"), "password must be removed: {sanitised}");
        assert!(sanitised.contains("user=alice"), "{sanitised}");
    }

    #[test]
    fn sanitise_url_removes_key() {
        let url = make_url("https://api.example.com/data?key=mykey&count=10");
        let sanitised = sanitise_url(&url);
        assert!(!sanitised.contains("key="), "key must be removed: {sanitised}");
        assert!(sanitised.contains("count=10"), "{sanitised}");
    }

    #[test]
    fn sanitise_url_case_insensitive_api_key() {
        let url = make_url("https://api.example.com/data?API_KEY=SECRET&other=1");
        let sanitised = sanitise_url(&url);
        assert!(!sanitised.contains("API_KEY"), "API_KEY must be removed: {sanitised}");
        assert!(sanitised.contains("other=1"), "{sanitised}");
    }

    #[test]
    fn sanitise_url_case_insensitive_token() {
        let url = make_url("https://api.example.com/data?Token=abc&x=1");
        let sanitised = sanitise_url(&url);
        assert!(!sanitised.contains("Token"), "Token must be removed: {sanitised}");
        assert!(sanitised.contains("x=1"), "{sanitised}");
    }

    #[test]
    fn sanitise_url_no_query_string_unchanged() {
        let url = make_url("https://api.example.com/v1/data");
        let sanitised = sanitise_url(&url);
        assert_eq!(sanitised, "https://api.example.com/v1/data");
    }

    #[test]
    fn sanitise_url_all_sensitive_removes_query_string() {
        let url = make_url("https://api.example.com/v1?api_key=a&token=b");
        let sanitised = sanitise_url(&url);
        assert!(!sanitised.contains('?'), "query string must be absent: {sanitised}");
    }

    // ── LogRecord serialisation ───────────────────────────────────────────────

    #[test]
    fn log_record_serialises_to_ndjson_with_correct_fields() {
        let server_id = Uuid::new_v4();
        let instance_id = Uuid::new_v4().to_string();
        let record = LogRecord {
            timestamp: "2026-03-30T12:00:00.000Z".to_string(),
            server_id,
            server_slug: "my-api".to_string(),
            method: "tools/call".to_string(),
            tool_name: Some("get_weather".to_string()),
            upstream_url: Some("https://api.example.com/weather?city=London".to_string()),
            upstream_status: Some(200),
            latency_ms: 42,
            upstream_latency_ms: Some(38),
            response_size_bytes: Some(512),
            transform_warnings: vec![],
            token_prefix: Some("abc12345".to_string()),
            instance_id: instance_id.clone(),
            trace_id: None,
        };

        let json = serde_json::to_string(&record).expect("serialisation must succeed");
        assert!(!json.contains('\n'), "NDJSON must not contain newlines: {json}");

        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("output must be valid JSON");
        assert_eq!(parsed["server_slug"], "my-api");
        assert_eq!(parsed["method"], "tools/call");
        assert_eq!(parsed["tool_name"], "get_weather");
        assert_eq!(parsed["upstream_status"], 200);
        assert_eq!(parsed["latency_ms"], 42);
        assert_eq!(parsed["upstream_latency_ms"], 38);
        assert_eq!(parsed["response_size_bytes"], 512);
        assert_eq!(parsed["instance_id"], instance_id.as_str());
        // None fields must be omitted (skip_serializing_if).
        assert!(
            parsed.get("trace_id").is_none(),
            "absent trace_id must be omitted from JSON"
        );
    }

    // ── RequestLogger ─────────────────────────────────────────────────────────

    #[cfg(test)]
    impl RequestLogger {
        /// Test-only constructor: supply a custom sender without spawning the
        /// background writer.
        fn with_sender(
            tx: mpsc::Sender<LogMsg>,
            instance_id: String,
            level: LogLevel,
        ) -> Arc<Self> {
            Arc::new(Self { tx, instance_id, level })
        }
    }

    #[tokio::test]
    async fn ten_records_can_be_submitted_without_error() {
        let instance_id = Uuid::new_v4().to_string();
        let logger = RequestLogger::new(instance_id, LogLevel::Info);
        let server_id = Uuid::new_v4();

        for i in 0..10_u64 {
            let record = logger.new_record(server_id, "test-server", "tools/list", i * 10);
            logger.log(record); // must not panic or block
        }
        // Give the background task time to drain.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn channel_full_drops_record_without_blocking() {
        let instance_id = Uuid::new_v4().to_string();
        // Capacity-1 channel, no background reader — fills immediately.
        let (tx, _rx) = mpsc::channel::<LogMsg>(1);
        let logger = RequestLogger::with_sender(tx, instance_id.clone(), LogLevel::Info);
        let server_id = Uuid::new_v4();

        // First record fills the channel.
        let filler = logger.new_record(server_id, "s", "tools/list", 0);
        logger.log(filler);

        // Second record must be dropped without blocking.
        let overflow = logger.new_record(server_id, "s", "tools/list", 1);
        let start = std::time::Instant::now();
        logger.log(overflow);
        let elapsed_ms = start.elapsed().as_millis();
        assert!(
            elapsed_ms < 50,
            "log() must not block; elapsed {elapsed_ms}ms"
        );
    }

    #[test]
    fn is_debug_enabled_distinguishes_levels() {
        let (tx, _rx) = mpsc::channel::<LogMsg>(1);
        let debug_logger =
            RequestLogger::with_sender(tx.clone(), "id".to_string(), LogLevel::Debug);
        let info_logger =
            RequestLogger::with_sender(tx, "id".to_string(), LogLevel::Info);

        assert!(debug_logger.is_debug_enabled(), "Debug level must enable debug");
        assert!(!info_logger.is_debug_enabled(), "Info level must not enable debug");
    }

    #[test]
    fn log_level_ordering_is_correct() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
    }

    #[test]
    fn log_level_from_str_or_default_parses_known_values() {
        assert_eq!(LogLevel::from_str_or_default("trace"), LogLevel::Trace);
        assert_eq!(LogLevel::from_str_or_default("debug"), LogLevel::Debug);
        assert_eq!(LogLevel::from_str_or_default("info"), LogLevel::Info);
        assert_eq!(LogLevel::from_str_or_default("warn"), LogLevel::Warn);
        assert_eq!(LogLevel::from_str_or_default("warning"), LogLevel::Warn);
        assert_eq!(LogLevel::from_str_or_default("error"), LogLevel::Error);
        assert_eq!(LogLevel::from_str_or_default("unknown"), LogLevel::Info);
    }

    // ── flush() ───────────────────────────────────────────────────────────────

    /// `flush()` resolves after all previously-enqueued records have been
    /// processed by the background writer.
    #[tokio::test]
    async fn flush_resolves_after_pending_records_are_written() {
        let instance_id = Uuid::new_v4().to_string();
        let logger = RequestLogger::new(instance_id, LogLevel::Info);
        let server_id = Uuid::new_v4();

        // Enqueue 20 records.
        for i in 0..20_u64 {
            let record = logger.new_record(server_id, "flush-server", "tools/list", i);
            logger.log(record);
        }

        // flush() must resolve within a reasonable timeout once all records
        // ahead of the sentinel have been processed.
        tokio::time::timeout(
            tokio::time::Duration::from_millis(500),
            logger.flush(),
        )
        .await
        .expect("flush() must resolve within 500 ms");
    }

    /// `flush()` on a closed channel does not panic or hang.
    #[tokio::test]
    async fn flush_on_closed_channel_does_not_panic() {
        let instance_id = Uuid::new_v4().to_string();
        // Spawn a writer that exits immediately.
        let (tx, mut rx) = mpsc::channel::<LogMsg>(1);
        // Drop the receiver so the channel appears closed to the sender.
        rx.close();

        let logger = RequestLogger::with_sender(tx, instance_id, LogLevel::Info);

        // Must return without panicking even though the channel is closed.
        tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            logger.flush(),
        )
        .await
        .expect("flush() on closed channel must resolve immediately");
    }
}
