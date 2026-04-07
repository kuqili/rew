//! Storage lifecycle manager with tiered retention policy.
//!
//! Implements the retention strategy defined in the technical spec:
//! - 0–1 hour: keep ALL snapshots
//! - 1–24 hours: keep 1 per hour (most recent in each hour slot)
//! - 1–30 days: keep 1 per day (most recent in each day slot)
//! - > 30 days: delete all
//!
//! Special rules:
//! - Anomaly-triggered snapshots: retention period multiplied by 2
//! - Pinned snapshots: never auto-deleted
//! - Disk usage threshold alerts (default: 10GB)

use crate::config::RetentionPolicyConfig;
use crate::db::Database;
use crate::error::RewResult;
use crate::snapshot::tmutil::TmutilWrapper;
use crate::types::{Snapshot, SnapshotTrigger};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Result of a cleanup operation.
#[derive(Debug, Clone)]
pub struct CleanupResult {
    /// Number of snapshots deleted.
    pub deleted_count: u32,
    /// Number of snapshots retained.
    pub retained_count: u32,
    /// Number of pinned snapshots skipped.
    pub pinned_skipped: u32,
    /// Number of anomaly snapshots with extended retention.
    pub anomaly_extended: u32,
    /// IDs of deleted snapshots.
    pub deleted_ids: Vec<Uuid>,
    /// Whether disk usage exceeds the threshold.
    pub disk_alert: bool,
}

/// Storage lifecycle manager.
///
/// Called after each new snapshot creation to enforce the retention policy.
/// Works with the database for metadata and TmutilWrapper for OS-level deletion.
pub struct StorageManager {
    db: Database,
    tmutil: TmutilWrapper,
    policy: RetentionPolicyConfig,
    /// Disk usage alert threshold in bytes (default: 10GB).
    disk_threshold_bytes: u64,
}

impl StorageManager {
    /// Create a new StorageManager with the given configuration.
    pub fn new(db: Database, tmutil: TmutilWrapper, policy: RetentionPolicyConfig) -> Self {
        Self {
            db,
            tmutil,
            policy,
            disk_threshold_bytes: 10 * 1024 * 1024 * 1024, // 10GB
        }
    }

    /// Create with a custom disk threshold.
    pub fn with_disk_threshold(mut self, threshold_bytes: u64) -> Self {
        self.disk_threshold_bytes = threshold_bytes;
        self
    }

    /// Run the cleanup process.
    ///
    /// This is the main entry point, called after each snapshot creation.
    /// It applies the tiered retention policy and deletes excess snapshots.
    pub fn run_cleanup(&self) -> RewResult<CleanupResult> {
        self.run_cleanup_at(Utc::now())
    }

    /// Run cleanup with a specific "now" time (for testing).
    pub fn run_cleanup_at(&self, now: DateTime<Utc>) -> RewResult<CleanupResult> {
        let all_snapshots = self.db.list_snapshots()?;
        info!("Running cleanup: {} total snapshots", all_snapshots.len());

        let mut deleted_ids = Vec::new();
        let mut anomaly_extended = 0u32;

        // Separate pinned snapshots — they are never deleted
        let (pinned, candidates): (Vec<_>, Vec<_>) =
            all_snapshots.into_iter().partition(|s| s.pinned);
        let pinned_skipped = pinned.len() as u32;

        // Classify candidates by retention tier
        let to_delete = self.apply_retention_policy(&candidates, now, &mut anomaly_extended);

        // Delete the snapshots
        for snapshot in &to_delete {
            match self.delete_snapshot(snapshot) {
                Ok(()) => {
                    deleted_ids.push(snapshot.id);
                }
                Err(e) => {
                    warn!(
                        "Failed to delete snapshot {}: {} (will retry next cycle)",
                        snapshot.id, e
                    );
                }
            }
        }

        let retained_count =
            (candidates.len() - deleted_ids.len()) as u32 + pinned_skipped;

        let result = CleanupResult {
            deleted_count: deleted_ids.len() as u32,
            retained_count,
            pinned_skipped,
            anomaly_extended,
            deleted_ids,
            disk_alert: self.check_disk_usage(),
        };

        info!(
            "Cleanup complete: deleted={}, retained={}, pinned_skipped={}, anomaly_extended={}",
            result.deleted_count, result.retained_count, result.pinned_skipped, result.anomaly_extended
        );

        Ok(result)
    }

    /// Apply the tiered retention policy to a list of snapshot candidates.
    ///
    /// Returns the list of snapshots that should be deleted.
    fn apply_retention_policy(
        &self,
        candidates: &[Snapshot],
        now: DateTime<Utc>,
        anomaly_extended: &mut u32,
    ) -> Vec<Snapshot> {
        let keep_all_cutoff = now - Duration::seconds(self.policy.keep_all_secs as i64);
        let keep_hourly_cutoff = now - Duration::seconds(self.policy.keep_hourly_secs as i64);
        let keep_daily_cutoff = now - Duration::seconds(self.policy.keep_daily_secs as i64);

        let anomaly_multiplier = self.policy.anomaly_retention_multiplier as i64;

        // Extended cutoffs for anomaly snapshots
        let anomaly_keep_all_cutoff =
            now - Duration::seconds(self.policy.keep_all_secs as i64 * anomaly_multiplier);
        let anomaly_keep_hourly_cutoff =
            now - Duration::seconds(self.policy.keep_hourly_secs as i64 * anomaly_multiplier);
        let anomaly_keep_daily_cutoff =
            now - Duration::seconds(self.policy.keep_daily_secs as i64 * anomaly_multiplier);

        let mut to_delete = Vec::new();

        // Tier 1: Within keep_all period — keep everything
        // (For anomaly snapshots, use the extended period)

        // Tier 2: Between keep_all and keep_hourly — keep 1 per hour
        let mut hourly_buckets: HashMap<String, Vec<Snapshot>> = HashMap::new();

        // Tier 3: Between keep_hourly and keep_daily — keep 1 per day
        let mut daily_buckets: HashMap<String, Vec<Snapshot>> = HashMap::new();

        for snap in candidates {
            let is_anomaly = snap.trigger == SnapshotTrigger::Anomaly;
            let (eff_keep_all, eff_keep_hourly, eff_keep_daily) = if is_anomaly {
                (
                    anomaly_keep_all_cutoff,
                    anomaly_keep_hourly_cutoff,
                    anomaly_keep_daily_cutoff,
                )
            } else {
                (keep_all_cutoff, keep_hourly_cutoff, keep_daily_cutoff)
            };

            if snap.timestamp > eff_keep_all {
                // Tier 1: keep all — do nothing
                if is_anomaly && snap.timestamp <= keep_all_cutoff {
                    *anomaly_extended += 1;
                }
                continue;
            } else if snap.timestamp > eff_keep_hourly {
                // Tier 2: keep 1 per hour
                if is_anomaly && snap.timestamp <= keep_hourly_cutoff {
                    *anomaly_extended += 1;
                }
                let hour_key = snap.timestamp.format("%Y-%m-%d-%H").to_string();
                hourly_buckets
                    .entry(hour_key)
                    .or_default()
                    .push(snap.clone());
            } else if snap.timestamp > eff_keep_daily {
                // Tier 3: keep 1 per day
                if is_anomaly && snap.timestamp <= keep_daily_cutoff {
                    *anomaly_extended += 1;
                }
                let day_key = snap.timestamp.format("%Y-%m-%d").to_string();
                daily_buckets
                    .entry(day_key)
                    .or_default()
                    .push(snap.clone());
            } else {
                // Tier 4: too old — delete
                to_delete.push(snap.clone());
            }
        }

        // For hourly buckets: keep the most recent in each hour, delete the rest
        for (_hour, mut snaps) in hourly_buckets {
            snaps.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            // Keep the first (most recent), delete the rest
            for snap in snaps.into_iter().skip(1) {
                to_delete.push(snap);
            }
        }

        // For daily buckets: keep the most recent in each day, delete the rest
        for (_day, mut snaps) in daily_buckets {
            snaps.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            for snap in snaps.into_iter().skip(1) {
                to_delete.push(snap);
            }
        }

        to_delete
    }

    /// Delete a single snapshot from both the database and the OS.
    fn delete_snapshot(&self, snapshot: &Snapshot) -> RewResult<()> {
        // Delete from OS
        if let Some(date) =
            TmutilWrapper::extract_date_from_name(&snapshot.os_snapshot_ref)
        {
            match self.tmutil.delete_snapshot(&date) {
                Ok(()) => {
                    debug!("Deleted OS snapshot: {}", snapshot.os_snapshot_ref);
                }
                Err(e) => {
                    warn!(
                        "Failed to delete OS snapshot {} (may already be gone): {}",
                        snapshot.os_snapshot_ref, e
                    );
                    // Continue to delete DB record
                }
            }
        }

        // Delete from database
        self.db.delete_snapshot(&snapshot.id)?;
        Ok(())
    }

    /// Check if disk usage by snapshots exceeds the configured threshold.
    ///
    /// Returns true if usage exceeds the threshold (alert condition).
    /// Uses a heuristic: counts snapshots × average snapshot overhead estimate.
    /// On macOS APFS, snapshots use CoW so actual disk usage depends on data changes.
    fn check_disk_usage(&self) -> bool {
        // Try to get actual volume available space via statvfs
        #[cfg(target_os = "macos")]
        {
            use std::ffi::CString;
            use std::mem::MaybeUninit;

            let volume = CString::new(self.tmutil.volume()).unwrap_or_default();
            if !volume.as_bytes().is_empty() {
                let mut stat = MaybeUninit::<libc::statvfs>::uninit();
                let ret = unsafe { libc::statvfs(volume.as_ptr(), stat.as_mut_ptr()) };
                if ret == 0 {
                    let stat = unsafe { stat.assume_init() };
                    let total_bytes = stat.f_blocks as u64 * stat.f_frsize as u64;
                    let avail_bytes = stat.f_bavail as u64 * stat.f_frsize as u64;
                    let used_bytes = total_bytes.saturating_sub(avail_bytes);

                    // If available space is less than the threshold, alert
                    if avail_bytes < self.disk_threshold_bytes {
                        warn!(
                            "Disk space alert: only {} MB available (threshold: {} MB)",
                            avail_bytes / (1024 * 1024),
                            self.disk_threshold_bytes / (1024 * 1024)
                        );
                        return true;
                    }

                    debug!(
                        "Disk usage: {} GB used, {} GB available, threshold: {} GB",
                        used_bytes / (1024 * 1024 * 1024),
                        avail_bytes / (1024 * 1024 * 1024),
                        self.disk_threshold_bytes / (1024 * 1024 * 1024)
                    );
                }
            }
        }

        false
    }

    /// Get a reference to the underlying database.
    pub fn db(&self) -> &Database {
        &self.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RetentionPolicyConfig;
    use crate::types::SnapshotTrigger;
    use chrono::Timelike;
    use tempfile::tempdir;

    fn test_storage_manager() -> (StorageManager, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        db.initialize().unwrap();
        let policy = RetentionPolicyConfig::default();
        let tmutil = TmutilWrapper::default();
        let manager = StorageManager::new(db, tmutil, policy);
        (manager, dir)
    }

    fn insert_snapshot_at(
        db: &Database,
        timestamp: DateTime<Utc>,
        trigger: SnapshotTrigger,
        pinned: bool,
    ) -> Snapshot {
        let snapshot = Snapshot {
            id: Uuid::new_v4(),
            timestamp,
            trigger,
            os_snapshot_ref: format!(
                "com.apple.TimeMachine.{}.local",
                timestamp.format("%Y-%m-%d-%H%M%S")
            ),
            files_added: 1,
            files_modified: 1,
            files_deleted: 0,
            pinned,
            metadata_json: None,
        };
        db.save_snapshot(&snapshot).unwrap();
        snapshot
    }

    #[test]
    fn test_cleanup_empty_db() {
        let (manager, _dir) = test_storage_manager();
        let result = manager.run_cleanup().unwrap();
        assert_eq!(result.deleted_count, 0);
        assert_eq!(result.retained_count, 0);
    }

    #[test]
    fn test_recent_snapshots_kept() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Insert 5 snapshots in the last 30 minutes — all should be kept
        for i in 0..5 {
            insert_snapshot_at(
                manager.db(),
                now - Duration::minutes(i * 5),
                SnapshotTrigger::Auto,
                false,
            );
        }

        let result = manager.run_cleanup_at(now).unwrap();
        assert_eq!(result.deleted_count, 0);
        assert_eq!(result.retained_count, 5);
    }

    #[test]
    fn test_hourly_retention_keeps_one_per_hour() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Insert 3 snapshots in the same calendar hour, 2 hours ago.
        // Use :00, :10, :20 within that hour to guarantee they're in the same bucket.
        let base_time = now - Duration::hours(2);
        // Truncate to the start of that hour to avoid calendar-hour boundary flakiness
        let hour_start = base_time
            .date_naive()
            .and_hms_opt(base_time.hour(), 0, 0)
            .unwrap()
            .and_utc();

        for i in 0..3i64 {
            insert_snapshot_at(
                manager.db(),
                hour_start + Duration::minutes(i * 10),
                SnapshotTrigger::Auto,
                false,
            );
        }

        let result = manager.run_cleanup_at(now).unwrap();
        assert_eq!(result.deleted_count, 2); // 2 of 3 deleted
        assert_eq!(result.retained_count, 1);

        // Verify the remaining one is the most recent in that hour
        let remaining = manager.db().list_snapshots().unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].timestamp >= hour_start + Duration::minutes(20));
    }

    #[test]
    fn test_daily_retention_keeps_one_per_day() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Insert 4 snapshots on the same day, 3 days ago
        let base_time = now - Duration::days(3);
        for i in 0..4 {
            insert_snapshot_at(
                manager.db(),
                base_time + Duration::hours(i * 2),
                SnapshotTrigger::Auto,
                false,
            );
        }

        let result = manager.run_cleanup_at(now).unwrap();
        assert_eq!(result.deleted_count, 3);
        assert_eq!(result.retained_count, 1);
    }

    #[test]
    fn test_old_snapshots_deleted() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Insert snapshots older than 30 days — all should be deleted
        for i in 0..5 {
            insert_snapshot_at(
                manager.db(),
                now - Duration::days(31 + i),
                SnapshotTrigger::Auto,
                false,
            );
        }

        let result = manager.run_cleanup_at(now).unwrap();
        assert_eq!(result.deleted_count, 5);
        assert_eq!(result.retained_count, 0);
    }

    #[test]
    fn test_pinned_snapshots_never_deleted() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Insert pinned snapshots that are very old
        for i in 0..3 {
            insert_snapshot_at(
                manager.db(),
                now - Duration::days(60 + i),
                SnapshotTrigger::Manual,
                true,
            );
        }

        // Also insert some old unpinned ones
        for i in 0..2 {
            insert_snapshot_at(
                manager.db(),
                now - Duration::days(60 + i),
                SnapshotTrigger::Auto,
                false,
            );
        }

        let result = manager.run_cleanup_at(now).unwrap();
        assert_eq!(result.pinned_skipped, 3);
        assert_eq!(result.deleted_count, 2);

        // Verify pinned ones are still in DB
        let remaining = manager.db().list_snapshots().unwrap();
        assert_eq!(remaining.len(), 3);
        assert!(remaining.iter().all(|s| s.pinned));
    }

    #[test]
    fn test_anomaly_snapshots_double_retention() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Default keep_all_secs = 3600 (1 hour)
        // For anomaly snapshots, keep_all extends to 2 hours (multiplier = 2)

        // Insert an anomaly snapshot 90 minutes ago
        // Normal: would be in hourly tier, but anomaly: still in keep_all tier
        let anomaly_snap = insert_snapshot_at(
            manager.db(),
            now - Duration::minutes(90),
            SnapshotTrigger::Anomaly,
            false,
        );

        // Insert a normal snapshot at the same time — should be in hourly tier
        let _normal_snap = insert_snapshot_at(
            manager.db(),
            now - Duration::minutes(90),
            SnapshotTrigger::Auto,
            false,
        );

        // Also insert another normal snapshot in the same hour
        let _normal_snap2 = insert_snapshot_at(
            manager.db(),
            now - Duration::minutes(95),
            SnapshotTrigger::Auto,
            false,
        );

        let result = manager.run_cleanup_at(now).unwrap();

        // The anomaly snap should be kept (extended keep_all)
        // Of the two normal snaps, one should be kept (hourly tier keeps 1 per hour)
        assert!(result.anomaly_extended >= 1);

        let remaining = manager.db().list_snapshots().unwrap();
        // anomaly snap should still exist
        assert!(remaining.iter().any(|s| s.id == anomaly_snap.id));
    }

    #[test]
    fn test_70_snapshots_retention_policy() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Create 70 test snapshots spread across different time periods:
        // 10 in last hour (kept: all 10)
        for i in 0..10 {
            insert_snapshot_at(
                manager.db(),
                now - Duration::minutes(i * 5),
                SnapshotTrigger::Auto,
                false,
            );
        }

        // 30 spread across the last 24 hours (1-24h range)
        // Each in a different hour-slot → up to 23 kept (one per hour for hours 1-23)
        for i in 0..30 {
            let hours_ago = 1 + (i % 23); // Spread across hours 1-23
            let minutes_offset = (i / 23) * 15; // Multiple per hour for some
            insert_snapshot_at(
                manager.db(),
                now - Duration::hours(hours_ago) - Duration::minutes(minutes_offset),
                SnapshotTrigger::Auto,
                false,
            );
        }

        // 20 spread across the last 30 days (1-30 day range)
        for i in 0..20 {
            let days_ago = 1 + (i % 29); // Spread across days 1-29
            insert_snapshot_at(
                manager.db(),
                now - Duration::days(days_ago) - Duration::hours(12),
                SnapshotTrigger::Auto,
                false,
            );
        }

        // 10 older than 30 days (all deleted)
        for i in 0..10 {
            insert_snapshot_at(
                manager.db(),
                now - Duration::days(31 + i),
                SnapshotTrigger::Auto,
                false,
            );
        }

        let before_count = manager.db().list_snapshots().unwrap().len();
        assert_eq!(before_count, 70);

        let result = manager.run_cleanup_at(now).unwrap();

        let after = manager.db().list_snapshots().unwrap();
        let after_count = after.len();

        // Verify the retention policy:
        // - All 10 recent snapshots should be kept (< 1 hour)
        // - Hourly tier: at most 23 kept (one per hour for hours 1-23)
        // - Daily tier: at most 29 kept (one per day for days 1-29)
        // - All 10 old snapshots should be deleted

        // At minimum, the 10 old ones should be gone
        assert!(result.deleted_count >= 10, "At least 10 old snapshots should be deleted");

        // Recent snapshots (< 1 hour) should all be kept
        let recent_count = after
            .iter()
            .filter(|s| s.timestamp > now - Duration::hours(1))
            .count();
        assert_eq!(recent_count, 10, "All recent snapshots should be kept");

        // Hourly tier: at most 1 per hour
        let hourly_snaps: Vec<_> = after
            .iter()
            .filter(|s| {
                s.timestamp <= now - Duration::hours(1)
                    && s.timestamp > now - Duration::hours(24)
            })
            .collect();
        // Check no two in the same hour
        let mut hourly_buckets: HashMap<String, u32> = HashMap::new();
        for s in &hourly_snaps {
            let key = s.timestamp.format("%Y-%m-%d-%H").to_string();
            *hourly_buckets.entry(key).or_default() += 1;
        }
        for (hour, count) in &hourly_buckets {
            assert!(
                *count <= 1,
                "Hour {} has {} snapshots, expected at most 1",
                hour,
                count
            );
        }
        assert!(hourly_snaps.len() <= 24, "Hourly tier should have at most 24");

        // Daily tier: at most 1 per day
        let daily_snaps: Vec<_> = after
            .iter()
            .filter(|s| {
                s.timestamp <= now - Duration::hours(24)
                    && s.timestamp > now - Duration::days(30)
            })
            .collect();
        let mut daily_buckets: HashMap<String, u32> = HashMap::new();
        for s in &daily_snaps {
            let key = s.timestamp.format("%Y-%m-%d").to_string();
            *daily_buckets.entry(key).or_default() += 1;
        }
        for (day, count) in &daily_buckets {
            assert!(
                *count <= 1,
                "Day {} has {} snapshots, expected at most 1",
                day,
                count
            );
        }
        assert!(daily_snaps.len() <= 30, "Daily tier should have at most 30");

        // Nothing older than 30 days should remain
        let very_old = after
            .iter()
            .filter(|s| s.timestamp < now - Duration::days(30))
            .count();
        assert_eq!(very_old, 0, "No snapshots older than 30 days should remain");

        info!(
            "70-snapshot test: before={}, after={}, deleted={}, \
             recent={}, hourly={}, daily={}",
            before_count,
            after_count,
            result.deleted_count,
            recent_count,
            hourly_snaps.len(),
            daily_snaps.len()
        );
    }

    #[test]
    fn test_pinned_snapshots_survive_30_days() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Create pinned snapshots at various old dates
        let pinned_ids: Vec<Uuid> = (0..5)
            .map(|i| {
                let snap = insert_snapshot_at(
                    manager.db(),
                    now - Duration::days(31 + i * 10), // 31, 41, 51, 61, 71 days ago
                    SnapshotTrigger::Manual,
                    true,
                );
                snap.id
            })
            .collect();

        // Create some unpinned old snapshots that should be deleted
        for i in 0..3 {
            insert_snapshot_at(
                manager.db(),
                now - Duration::days(35 + i * 5),
                SnapshotTrigger::Auto,
                false,
            );
        }

        let result = manager.run_cleanup_at(now).unwrap();

        // All unpinned old ones deleted
        assert_eq!(result.deleted_count, 3);
        assert_eq!(result.pinned_skipped, 5);

        // All pinned snapshots still exist
        let remaining = manager.db().list_snapshots().unwrap();
        assert_eq!(remaining.len(), 5);
        for id in &pinned_ids {
            assert!(
                remaining.iter().any(|s| s.id == *id),
                "Pinned snapshot {} should survive cleanup",
                id
            );
        }
    }

    #[test]
    fn test_mixed_tiers_comprehensive() {
        let (manager, _dir) = test_storage_manager();
        let now = Utc::now();

        // Tier 1: 3 snapshots in last 30 minutes
        for i in 0..3 {
            insert_snapshot_at(
                manager.db(),
                now - Duration::minutes(i * 10),
                SnapshotTrigger::Auto,
                false,
            );
        }

        // Tier 2: 6 snapshots, 2 per hour for 3 hours (3-5 hours ago)
        // Use small offsets (5 and 10 min) to ensure both land in the same calendar hour
        for h in 3..6i64 {
            for m in &[5i64, 10] {
                insert_snapshot_at(
                    manager.db(),
                    now - Duration::hours(h) - Duration::minutes(*m),
                    SnapshotTrigger::Auto,
                    false,
                );
            }
        }

        // Tier 3: 4 snapshots, 2 per day for 2 days (5-6 days ago)
        // Use offsets that land on the same calendar day
        for d in 5..7i64 {
            for h in &[2i64, 3] {
                insert_snapshot_at(
                    manager.db(),
                    now - Duration::days(d) - Duration::hours(*h),
                    SnapshotTrigger::Auto,
                    false,
                );
            }
        }

        // 1 pinned old snapshot
        insert_snapshot_at(
            manager.db(),
            now - Duration::days(45),
            SnapshotTrigger::Manual,
            true,
        );

        // Total: 3 + 6 + 4 + 1 = 14
        assert_eq!(manager.db().list_snapshots().unwrap().len(), 14);

        let result = manager.run_cleanup_at(now).unwrap();

        // Expected:
        // Tier 1: 3 kept (all)
        // Tier 2: 3 kept (1 per hour × 3 hours), 3 deleted
        // Tier 3: 2 kept (1 per day × 2 days), 2 deleted
        // Pinned: 1 kept
        // Total kept: 3 + 3 + 2 + 1 = 9
        // Total deleted: 3 + 2 = 5
        assert_eq!(result.deleted_count, 5);
        assert_eq!(result.pinned_skipped, 1);

        let remaining = manager.db().list_snapshots().unwrap();
        assert_eq!(remaining.len(), 9);
    }
}
