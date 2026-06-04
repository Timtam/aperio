//! Fire-and-forget playback for custom notification sounds (§14.4).
//!
//! `tauri-plugin-notification` can only request a NAMED system sound or
//! silence — it can't play an arbitrary file. So for
//! [`cal_core::SoundSource::Custom`] we fire a *silent* notification and
//! play the audio file ourselves through this module.
//!
//! ## Why a dedicated thread owns the stream
//!
//! `rodio::OutputStream` must stay alive for the whole duration of any
//! playback driven from it, and it isn't `Send` on every platform.
//! Rather than thread its lifetime through the async scheduler, we park
//! it on one long-lived OS thread that owns the stream for the entire
//! process and receives "play this path" messages over a channel. The
//! handle we hand out is just a [`std::sync::mpsc::SyncSender`] — which
//! is `Send + Sync`, so the handle drops cleanly into Tauri State and
//! into the `Arc<ReminderScheduler>` shared across the async runtime.
//!
//! Playback is serialised on that one thread (each clip plays to the
//! end before the next starts). Reminders almost never fire at the same
//! instant, and serialising avoids two clips talking over each other.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::thread;

use tracing::{debug, warn};

/// How many pending playback requests we buffer before dropping new
/// ones. A reminder storm large enough to fill this is already past the
/// point where more overlapping audio helps the user.
const QUEUE_DEPTH: usize = 32;

/// Cloneable handle to the audio thread. Sending a path queues it for
/// playback; all sends are non-blocking and best-effort.
#[derive(Clone)]
pub struct AudioPlayer {
    tx: SyncSender<PathBuf>,
}

impl AudioPlayer {
    /// Spawn the dedicated audio thread. The `OutputStream` is created
    /// inside the thread and lives as long as it does (the whole
    /// process), so sinks made from it never outlive their stream.
    pub fn spawn() -> Self {
        let (tx, rx) = sync_channel::<PathBuf>(QUEUE_DEPTH);
        thread::Builder::new()
            .name("aperio-audio".into())
            .spawn(move || audio_thread(rx))
            .expect("failed to spawn audio thread");
        Self { tx }
    }

    /// Queue `path` for playback. Fire-and-forget: a full queue or a
    /// gone thread is logged, never fatal — the visual notification has
    /// already shown by the time we get here.
    pub fn play_file(&self, path: PathBuf) {
        match self.tx.try_send(path) {
            Ok(()) => {}
            Err(TrySendError::Full(p)) => {
                warn!(path = ?p, "audio queue full; dropping playback request")
            }
            Err(TrySendError::Disconnected(p)) => {
                warn!(path = ?p, "audio thread gone; dropping playback request")
            }
        }
    }
}

/// Thread body: own the output stream, play each incoming path to the
/// end. Exits when the last [`AudioPlayer`] handle drops (channel
/// closes).
fn audio_thread(rx: Receiver<PathBuf>) {
    // Acquire the default output device once. On a headless box (CI) or
    // a machine with no audio, this fails — we then drain the channel as
    // no-ops so senders never block on a full queue.
    let (_stream, handle) = match rodio::OutputStream::try_default() {
        Ok(pair) => pair,
        Err(err) => {
            warn!(
                ?err,
                "no audio output device; custom sounds disabled this session"
            );
            for _ in rx { /* drain so senders never block */ }
            return;
        }
    };
    for path in rx {
        match play_one(&handle, &path) {
            Ok(()) => debug!(?path, "played custom notification sound"),
            Err(err) => warn!(?path, %err, "failed to play custom notification sound"),
        }
    }
}

/// Decode and play a single file to completion. Errors (missing file,
/// unsupported codec, decode failure) bubble up as a string for the
/// caller to log — they never poison the thread.
fn play_one(handle: &rodio::OutputStreamHandle, path: &Path) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let source =
        rodio::Decoder::new(std::io::BufReader::new(file)).map_err(|e| format!("decode: {e}"))?;
    let sink = rodio::Sink::try_new(handle).map_err(|e| format!("sink: {e}"))?;
    sink.append(source);
    // Block this dedicated thread (not the caller) until the clip ends,
    // so queued reminders play one after another instead of cutting
    // each other off.
    sink.sleep_until_end();
    Ok(())
}
