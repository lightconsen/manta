//! Pruning of old turn observability data.
//!
//! Shared by `syscity observe prune` and the daemon-startup auto-cleanup
//! (`observe.retention_days`). JSON date directories are removed by
//! lexicographic date comparison (YYYY-MM-DD names sort correctly); SQLite
//! metric rows are removed by the store's `delete_metrics_before` using
//! [`cutoff_ms`].

use std::path::Path;

/// Local-date string (YYYY-MM-DD) `days` days ago. Date-directory names sort
/// lexicographically, so string comparison is a valid "older than" test.
pub fn cutoff_date(days: u32) -> String {
    (chrono::Local::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%d")
        .to_string()
}

/// Epoch milliseconds of local midnight `days` days ago. Metric rows with
/// `started_at <` this value are strictly older than `days` calendar days.
pub fn cutoff_ms(days: u32) -> i64 {
    let now = chrono::Local::now();
    let date = (now - chrono::Duration::days(days as i64)).date_naive();
    let Some(midnight) = date.and_hms_opt(0, 0, 0) else {
        return now.timestamp_millis();
    };
    match midnight.and_local_timezone(chrono::Local) {
        chrono::LocalResult::Single(d) => d.timestamp_millis(),
        chrono::LocalResult::Ambiguous(d, _) => d.timestamp_millis(),
        chrono::LocalResult::None => now.timestamp_millis(),
    }
}

/// Remove date directories under `base` whose names sort strictly before
/// `cutoff` (YYYY-MM-DD). Returns `(removed_dirs, removed_files)`.
pub fn prune_turn_dirs(base: &Path, cutoff: &str) -> (usize, usize) {
    let mut dirs = 0;
    let mut files = 0;
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return (dirs, files),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Only consider YYYY-MM-DD directories; a directory exactly at the
        // cutoff is kept (it is not yet strictly older than `days` days).
        if name.len() != 10 || name.chars().nth(4) != Some('-') || name.as_str() >= cutoff {
            continue;
        }
        if let Ok(count) = std::fs::read_dir(&path) {
            files += count.flatten().count();
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            dirs += 1;
        }
    }
    (dirs, files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;
    use tempfile::TempDir;

    fn write_dirs(base: &std::path::Path, names: &[&str]) {
        for n in names {
            let dir = base.join(n);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("t.json"), "{}").unwrap();
        }
    }

    #[test]
    fn cutoff_date_is_valid_yyyymmdd() {
        let s = cutoff_date(30);
        assert_eq!(s.len(), 10);
        assert_eq!(s.as_bytes()[4], b'-');
        assert_eq!(s.as_bytes()[7], b'-');
    }

    #[test]
    fn prune_removes_strictly_older_dirs_only() {
        let dir = TempDir::new().unwrap();
        write_dirs(
            dir.path(),
            &[
                "2026-07-01",
                "2026-07-15",
                "2026-08-14", // == cutoff, kept
                "2026-08-15",
                "not-a-date",
            ],
        );
        let (dirs, files) = prune_turn_dirs(dir.path(), "2026-08-14");
        assert_eq!(dirs, 2); // 07-01 and 07-15
        assert_eq!(files, 2);
        assert!(dir.path().join("2026-08-14").exists());
        assert!(dir.path().join("2026-08-15").exists());
        assert!(dir.path().join("not-a-date").exists());
    }

    #[test]
    fn prune_missing_base_is_noop() {
        let dir = TempDir::new().unwrap();
        let gone = dir.path().join("does-not-exist");
        let (dirs, files) = prune_turn_dirs(&gone, "2026-08-14");
        assert_eq!((dirs, files), (0, 0));
    }

    #[test]
    fn cutoff_ms_matches_cutoff_date_midnight() {
        // Both helpers derive from the same local "now - N days", so a record
        // stored on the cutoff date must never be pruned by either.
        let days = 30;
        let ms = cutoff_ms(days);
        let date = chrono::DateTime::from_timestamp_millis(ms)
            .unwrap()
            .with_timezone(&chrono::Local);
        assert_eq!(date.format("%Y-%m-%d").to_string(), cutoff_date(days));
        assert_eq!(date.hour(), 0);
        assert_eq!(date.minute(), 0);
    }
}
