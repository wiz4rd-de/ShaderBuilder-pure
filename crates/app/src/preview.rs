//! Preview frame transport: a `tauri::ipc::Channel` streaming raw RGBA frames
//! from Rust to the webview `<canvas>` (Architecture §E/§F, Decision Log #15).
//!
//! The transport is deliberately decoupled from what produces the frames. Phase
//! 0 drives it with [`preview_engine::GradientSource`] (a dummy animated
//! gradient); Phase 1 swaps in the offscreen wgpu renderer behind the same
//! [`preview_engine::FrameSource`] trait — **nothing in this file changes**.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use preview_engine::{FrameSource, GradientSource};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

/// Default preview resolution. Small and fixed: transfer cost is bounded by the
/// pane size, not the simulated viewport (Architecture §F).
const DEFAULT_PREVIEW_WIDTH: u32 = 512;
const DEFAULT_PREVIEW_HEIGHT: u32 = 384;
/// Frame period for ~60 fps.
const FRAME_PERIOD: Duration = Duration::from_micros(16_667);

/// The single active preview stream: a caller-supplied id and its stop flag.
struct ActiveStream {
    id: String,
    running: Arc<AtomicBool>,
}

/// Managed state holding the single active preview stream, if any.
#[derive(Default)]
pub struct PreviewState {
    active: Mutex<Option<ActiveStream>>,
}

impl PreviewState {
    /// Stop whatever stream is active (used when a new one starts).
    fn stop_any(&self) {
        if let Some(stream) = self.active.lock().unwrap().take() {
            stream.running.store(false, Ordering::Relaxed);
        }
    }

    /// Stop the active stream only if its id matches `id`. A stale `stop` for a
    /// superseded stream is then a no-op — which is what makes start/stop robust
    /// to out-of-order IPC (e.g. React StrictMode's mount→unmount→mount, where
    /// the first unmount's stop can arrive after the second mount's start).
    fn stop_matching(&self, id: &str) {
        let mut guard = self.active.lock().unwrap();
        if guard.as_ref().is_some_and(|s| s.id == id) {
            guard
                .take()
                .unwrap()
                .running
                .store(false, Ordering::Relaxed);
        }
    }
}

/// Start streaming preview frames over `channel` at ~60 fps. Any previously
/// running stream is stopped first, so at most one producer runs at a time.
/// `stream_id` correlates this stream with its later `stop_preview_stream` call.
///
/// Frames are sent as raw binary ([`InvokeResponseBody::Raw`]), not JSON; the
/// frontend parses the documented header and blits to a `<canvas>`.
#[tauri::command]
pub fn start_preview_stream(
    state: State<'_, PreviewState>,
    channel: Channel<InvokeResponseBody>,
    stream_id: String,
    width: Option<u32>,
    height: Option<u32>,
) {
    state.stop_any();

    let running = Arc::new(AtomicBool::new(true));
    *state.active.lock().unwrap() = Some(ActiveStream {
        id: stream_id,
        running: running.clone(),
    });

    let width = width.unwrap_or(DEFAULT_PREVIEW_WIDTH);
    let height = height.unwrap_or(DEFAULT_PREVIEW_HEIGHT);

    std::thread::spawn(move || {
        // --- Dummy producer (Phase 0). Swapped for the wgpu renderer in Phase 1. ---
        let producer = GradientSource::new(width, height);
        // ---------------------------------------------------------------------------
        pump_frames(&channel, producer, &running, FRAME_PERIOD);
    });
}

/// Drive a [`FrameSource`], sending each rendered frame over `channel` as raw
/// binary, paced to `period`, until `running` is cleared or the channel closes.
///
/// Extracted from [`start_preview_stream`] so the transport can be unit-tested
/// headlessly (no Tauri runtime) — see the tests below.
fn pump_frames<S: FrameSource>(
    channel: &Channel<InvokeResponseBody>,
    mut source: S,
    running: &AtomicBool,
    period: Duration,
) {
    let mut frame_index: u64 = 0;
    let mut next = Instant::now();
    let mut buf = Vec::new();

    while running.load(Ordering::Relaxed) {
        source.render_into(frame_index, &mut buf);
        // `send` takes ownership; hand over this frame and start a fresh buffer.
        if channel
            .send(InvokeResponseBody::Raw(std::mem::take(&mut buf)))
            .is_err()
        {
            // The webview/channel went away — stop cleanly.
            break;
        }
        frame_index = frame_index.wrapping_add(1);

        // Frame pacing: sleep until the next deadline; if we've fallen behind,
        // resync rather than accumulating drift.
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    running.store(false, Ordering::Relaxed);
}

/// Stop the preview stream with the given id (idempotent; a non-matching or
/// already-stopped id is a no-op).
#[tauri::command]
pub fn stop_preview_stream(state: State<'_, PreviewState>, stream_id: String) {
    state.stop_matching(&stream_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use preview_engine::{FRAME_HEADER_LEN, FRAME_MAGIC};

    /// End-to-end transport check, no Tauri runtime: a `Channel` built from a
    /// collecting closure receives exactly the raw frames the producer sends.
    #[test]
    fn pump_sends_raw_frames_until_stopped() {
        let frames: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicBool::new(true));

        let sink = frames.clone();
        let stop = running.clone();
        let channel = Channel::new(move |body: InvokeResponseBody| {
            // The transport must carry *binary*, never JSON.
            let InvokeResponseBody::Raw(bytes) = body else {
                panic!("expected a raw binary frame, got JSON");
            };
            let mut got = sink.lock().unwrap();
            got.push(bytes);
            if got.len() >= 3 {
                stop.store(false, Ordering::Relaxed); // stop after a few frames
            }
            Ok(())
        });

        // period = ZERO so the test doesn't actually sleep.
        pump_frames(
            &channel,
            GradientSource::new(4, 4),
            &running,
            Duration::ZERO,
        );

        // The producer stopped once the consumer cleared the flag.
        assert!(
            !running.load(Ordering::Relaxed),
            "pump must exit when stopped"
        );

        let frames = frames.lock().unwrap();
        assert!(frames.len() >= 3, "expected at least 3 frames");
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(&frame[0..4], &FRAME_MAGIC, "frame {i} magic");
            assert_eq!(frame.len(), FRAME_HEADER_LEN + 4 * 4 * 4, "frame {i} size");
            // Frame index is written little-endian at offset 16.
            assert_eq!(frame[16] as usize, i, "frame {i} index in header");
        }
    }
}
