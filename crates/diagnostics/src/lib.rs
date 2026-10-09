//! Logging: `tracing` to stderr (text or JSON) plus a bounded in-memory
//! buffer that the desktop UI shows under Settings -> Diagnostics.
//! Secrets are never passed to log macros in this code base; this module
//! does no redaction of its own.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

#[derive(Clone, Debug, Serialize)]
pub struct LogLine {
    pub unix_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Clone, Default)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<LogLine>>>,
}

impl LogBuffer {
    const CAPACITY: usize = 2000;

    pub fn snapshot(&self) -> Vec<LogLine> {
        self.lines.lock().iter().cloned().collect()
    }

    fn push(&self, line: LogLine) {
        let mut l = self.lines.lock();
        if l.len() == Self::CAPACITY {
            l.pop_front();
        }
        l.push_back(line);
    }
}

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.insert_str(0, &format!("{value:?}"));
        } else {
            self.0.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.insert_str(0, value);
        } else {
            self.0.push_str(&format!(" {}={value}", field.name()));
        }
    }
}

struct BufferLayer(LogBuffer);

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut v = MessageVisitor(String::new());
        event.record(&mut v);
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.0.push(LogLine {
            unix_ms,
            level: event.metadata().level().to_string(),
            target: event.metadata().target().to_string(),
            message: v.0,
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

/// Install the global subscriber. `MIST_LOG` overrides the filter
/// (default: info for mist crates, warn elsewhere).
pub fn init(format: LogFormat) -> LogBuffer {
    let buffer = LogBuffer::default();
    let filter = EnvFilter::try_from_env("MIST_LOG").unwrap_or_else(|_| {
        EnvFilter::new("warn,mist=info,mist_app_core=info,mist_media_engine=info,mist_signaling_client=info,mist_input_api=info,mist_controller_api=info,mist_capture_api=info,mist_session_manager=info")
    });
    let fmt = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let registry = tracing_subscriber::registry().with(filter).with(BufferLayer(buffer.clone()));
    let result = match format {
        LogFormat::Text => registry.with(fmt).try_init(),
        LogFormat::Json => registry.with(fmt.json()).try_init(),
    };
    if result.is_err() {
        // Already initialized (e.g. in tests); keep the existing subscriber.
    }
    buffer
}
