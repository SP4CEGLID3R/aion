use std::time::{SystemTime, UNIX_EPOCH};

use radar_core::Millis;

/// Wall-clock milliseconds since the Unix epoch.
pub fn now_ms() -> Millis {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as Millis)
        .unwrap_or(0)
}
