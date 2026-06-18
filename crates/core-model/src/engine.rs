//! Engine ↔ frontend status & error events (#62).
//!
//! The headless wgpu render thread runs decoupled from the webview behind the
//! frame transport (Architecture §F): until #62 a render/compile failure only
//! reached stderr or an opaque `Result<(), String>`. These types are the TYPED
//! payload of a non-blocking event channel from the render thread to the
//! frontend, so the editor can surface a *recoverable* error (problems panel /
//! toast) and a live/last-good/stopped status instead of a silent stall.
//!
//! They live in `core-model` (the one shared serde + `#[ts(export)]` schema, §A)
//! so the Rust render thread, the app's Tauri event emitter, and the React
//! ingest all share one shape that can never drift. Both variants are emitted
//! over a single `engine-event` Tauri event as the tagged [`EngineEvent`] union.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Severity of an [`EngineErrorEvent`] (#62): does it stop rendering, or is it a
/// transient/advisory problem the app can keep running through?
///
/// An [`Error`](EngineSeverity::Error) means this frame/pass failed (a slang
/// compile error, an FBO allocation failure); a [`Warning`](EngineSeverity::Warning)
/// is advisory. Mirrors [`crate::ir::DiagnosticSeverity`] but is a distinct type
/// because engine events also carry the engine-specific [`EngineStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EngineSeverity {
    /// A blocking failure — this pass/frame did not render.
    Error,
    /// An advisory problem — rendering continues (possibly on last-good output).
    Warning,
}

/// A structured render/engine error surfaced from the render thread (#62).
///
/// Produced when the renderer cannot produce a fresh frame: a slang/glslang
/// compile failure for an in-memory chain pass (mapped to its owning pass), an
/// FBO/device allocation failure, or a device-lost. The frontend synthesizes a
/// diagnostic into its problems list (pass/node tagged when `pass_id`/`node_id`
/// are present) and shows a non-blocking toast for transient cases.
///
/// `code` is a short stable machine-readable tag (`"slangCompile"`,
/// `"deviceLost"`, `"fboAlloc"`, `"readback"`); `message` is human-readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EngineErrorEvent {
    /// Whether this stops the affected pass/frame or is advisory.
    pub severity: EngineSeverity,
    /// A short, stable, machine-readable category tag (e.g. `"slangCompile"`).
    pub code: String,
    /// The human-readable explanation of the problem.
    pub message: String,
    /// The pipeline pass id this error is about, when it maps to one (e.g. a
    /// whole-pass slang compile failure). `None` for pipeline-wide failures.
    ///
    /// Always serialized (`null` when `None`) so the wire shape and the generated
    /// TS type stay identical (the [`crate::ir::Diagnostic::port`] precedent).
    #[serde(default)]
    pub pass_id: Option<String>,
    /// The offending editor node id, when the error maps to one. `None` for
    /// pass-level or pipeline-wide failures.
    #[serde(default)]
    pub node_id: Option<String>,
}

impl EngineErrorEvent {
    /// Build an [`Error`](EngineSeverity::Error)-severity engine error.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: EngineSeverity::Error,
            code: code.into(),
            message: message.into(),
            pass_id: None,
            node_id: None,
        }
    }

    /// Build a [`Warning`](EngineSeverity::Warning)-severity engine error.
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: EngineSeverity::Warning,
            code: code.into(),
            message: message.into(),
            pass_id: None,
            node_id: None,
        }
    }

    /// Attach the owning pass id (builder style).
    #[must_use]
    pub fn with_pass(mut self, pass_id: impl Into<String>) -> Self {
        self.pass_id = Some(pass_id.into());
        self
    }

    /// Attach the offending node id (builder style).
    #[must_use]
    pub fn with_node(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }
}

/// The render engine's liveness state (#62), surfaced to the preview pane so the
/// user can tell a fresh render from a held last-good frame from a stopped engine.
///
/// `render_into` already emits a "waiting"/last-good frame on a not-ready/error
/// frame (the engine never tears down on a bad chain) — this names that state so
/// the preview can badge it instead of silently showing stale pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EngineStatus {
    /// A fresh frame was rendered this tick — the preview is live.
    Live,
    /// The latest render could not produce a fresh frame; the pane is showing the
    /// last good output (or the neutral waiting frame before the first render).
    LastGood,
    /// The render thread has stopped (no adapter / device lost / stream ended):
    /// no further frames will arrive until the stream restarts.
    Stopped,
}

/// A status/error event from the render thread to the frontend (#62), carried over
/// a single `engine-event` Tauri event as a tagged union.
///
/// [`Status`](EngineEvent::Status) reports the engine's liveness transitions
/// (live ↔ last-good ↔ stopped); [`Error`](EngineEvent::Error) reports a structured
/// render/compile failure. Kept as ONE event type (tagged by `kind`) so the
/// frontend registers a single listener and the wire shape is fixed.
///
/// Every event carries the `stream_id` of the preview stream that produced it (the
/// id the frontend passed to `start_preview_stream`). The frontend IGNORES events
/// from a superseded stream so a stopped/torn-down old render thread can neither
/// raise a spurious toast nor clobber the new stream's live status (#12, #13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum EngineEvent {
    /// A liveness-status transition (live / last-good / stopped).
    Status {
        /// The owning preview stream's id.
        #[serde(rename = "streamId")]
        stream_id: String,
        /// The new engine status.
        status: EngineStatus,
    },
    /// A structured render/compile error.
    Error {
        /// The owning preview stream's id.
        #[serde(rename = "streamId")]
        stream_id: String,
        /// The error payload.
        error: EngineErrorEvent,
    },
}

impl EngineEvent {
    /// Wrap a status into a [`Status`](EngineEvent::Status) event for `stream_id`.
    pub fn status(stream_id: impl Into<String>, status: EngineStatus) -> Self {
        EngineEvent::Status {
            stream_id: stream_id.into(),
            status,
        }
    }

    /// Wrap an error into an [`Error`](EngineEvent::Error) event for `stream_id`.
    pub fn error(stream_id: impl Into<String>, error: EngineErrorEvent) -> Self {
        EngineEvent::Error {
            stream_id: stream_id.into(),
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_event_serializes_tagged_camel_case() {
        let json =
            serde_json::to_string(&EngineEvent::status("s1", EngineStatus::LastGood)).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"status","streamId":"s1","status":"lastGood"}"#
        );
    }

    #[test]
    fn error_event_serializes_with_pass_and_node() {
        let ev = EngineEvent::error(
            "s1",
            EngineErrorEvent::error("slangCompile", "boom")
                .with_pass("pass-1")
                .with_node("node-2"),
        );
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"error","streamId":"s1","error":{"severity":"error","code":"slangCompile","message":"boom","passId":"pass-1","nodeId":"node-2"}}"#
        );
    }

    #[test]
    fn error_event_omitted_ids_serialize_as_null() {
        let ev = EngineEvent::error("s1", EngineErrorEvent::warning("deviceLost", "lost"));
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""passId":null"#));
        assert!(json.contains(r#""nodeId":null"#));
        // Round-trips back.
        let back: EngineEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }
}
