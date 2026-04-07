//! Core data types for rew.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// The kind of file system event observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileEventKind {
    Created,
    Modified,
    Deleted,
    Renamed,
}

/// A single file system event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEvent {
    /// Absolute path of the affected file
    pub path: PathBuf,
    /// Type of change
    pub kind: FileEventKind,
    /// When the event was observed
    pub timestamp: DateTime<Utc>,
    /// File size in bytes (if available)
    pub size_bytes: Option<u64>,
}

/// A batch of events aggregated within a sliding time window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBatch {
    /// The events in this batch
    pub events: Vec<FileEvent>,
    /// Start of the aggregation window
    pub window_start: DateTime<Utc>,
    /// End of the aggregation window
    pub window_end: DateTime<Utc>,
}

impl EventBatch {
    /// Count of events by kind within this batch.
    pub fn count_by_kind(&self, kind: &FileEventKind) -> usize {
        self.events.iter().filter(|e| &e.kind == kind).count()
    }

    /// Total size of deleted files in this batch.
    pub fn total_deleted_size(&self) -> u64 {
        self.events
            .iter()
            .filter(|e| e.kind == FileEventKind::Deleted)
            .filter_map(|e| e.size_bytes)
            .sum()
    }
}

/// What triggered a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotTrigger {
    /// Automatic periodic snapshot
    Auto,
    /// Triggered by anomaly detection
    Anomaly,
    /// User-initiated manual snapshot
    Manual,
}

impl std::fmt::Display for SnapshotTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotTrigger::Auto => write!(f, "auto"),
            SnapshotTrigger::Anomaly => write!(f, "anomaly"),
            SnapshotTrigger::Manual => write!(f, "manual"),
        }
    }
}

impl std::str::FromStr for SnapshotTrigger {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(SnapshotTrigger::Auto),
            "anomaly" => Ok(SnapshotTrigger::Anomaly),
            "manual" => Ok(SnapshotTrigger::Manual),
            _ => Err(format!("Unknown snapshot trigger: {}", s)),
        }
    }
}

/// Metadata for a single snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Unique identifier
    pub id: Uuid,
    /// When the snapshot was created
    pub timestamp: DateTime<Utc>,
    /// What triggered the snapshot
    pub trigger: SnapshotTrigger,
    /// Reference to the OS-level snapshot (e.g. APFS snapshot name)
    pub os_snapshot_ref: String,
    /// Number of files added since last snapshot
    pub files_added: u32,
    /// Number of files modified since last snapshot
    pub files_modified: u32,
    /// Number of files deleted since last snapshot
    pub files_deleted: u32,
    /// Whether the user has pinned this snapshot (permanent retention)
    pub pinned: bool,
    /// Additional metadata as JSON
    pub metadata_json: Option<String>,
}

/// Severity level for anomaly signals.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AnomalySeverity {
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for AnomalySeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnomalySeverity::Medium => write!(f, "MEDIUM"),
            AnomalySeverity::High => write!(f, "HIGH"),
            AnomalySeverity::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// The kind of anomaly detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnomalyKind {
    /// Bulk file deletion (RULE-01, RULE-02)
    BulkDelete,
    /// Large total deletion size (RULE-03)
    LargeDeletion,
    /// Bulk file modification (RULE-04)
    BulkModify,
    /// Root watch directory deleted (RULE-05)
    RootDirDeleted,
    /// Sensitive config file modified (RULE-06)
    SensitiveConfigModified,
    /// Large non-package modification (RULE-07)
    LargeNonPackageModify,
}

/// An anomaly signal emitted by the detection engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalySignal {
    /// Which anomaly rule was triggered
    pub kind: AnomalyKind,
    /// How severe is this anomaly
    pub severity: AnomalySeverity,
    /// Files that triggered this anomaly
    pub affected_files: Vec<PathBuf>,
    /// When the anomaly was detected
    pub detected_at: DateTime<Utc>,
    /// Human-readable description
    pub description: String,
}

/// A restore job describes what to restore and how.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreJob {
    /// Which snapshot to restore from
    pub snapshot_id: Uuid,
    /// Specific paths to restore (empty = full restore)
    pub target_paths: Vec<PathBuf>,
    /// If true, preview changes without applying
    pub dry_run: bool,
}
