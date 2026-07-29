//! Time utilities.

/// Millisecond UNIX timestamp for human-readable, sortable filenames.
pub fn ms_timestamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
