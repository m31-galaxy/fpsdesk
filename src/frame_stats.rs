//! A periodic readout of how long frames take.

use std::time::{Duration, Instant};

use tracing::debug;

/// How often to report.
const INTERVAL: Duration = Duration::from_secs(5);

/// Collects per-frame work times and logs a summary at debug level every few
/// seconds. Enable with `RUST_LOG=info,smithay=warn,fpsdesk::frame_stats=debug`.
pub struct FrameStats {
    since: Instant,
    frames: u32,
    total: Duration,
    worst: Duration,
}

impl FrameStats {
    pub fn new() -> Self {
        Self {
            since: Instant::now(),
            frames: 0,
            total: Duration::ZERO,
            worst: Duration::ZERO,
        }
    }

    /// Record a frame whose work (everything from updating the world to
    /// handing the finished frame to the display) took `work`.
    pub fn frame(&mut self, work: Duration) {
        self.frames += 1;
        self.total += work;
        self.worst = self.worst.max(work);

        let elapsed = self.since.elapsed();
        if elapsed >= INTERVAL {
            debug!(
                fps = format_args!("{:.1}", self.frames as f64 / elapsed.as_secs_f64()),
                mean_ms = format_args!(
                    "{:.2}",
                    self.total.as_secs_f64() * 1000.0 / self.frames as f64
                ),
                worst_ms = format_args!("{:.2}", self.worst.as_secs_f64() * 1000.0),
                "frame times"
            );
            *self = Self::new();
        }
    }
}
