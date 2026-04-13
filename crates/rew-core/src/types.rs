//! Core data types for rew.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// The kind of file system event observed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Operation outside project scope (RULE-08)
    OutOfScope,
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

// ============================================================
// Task-level types (V2: Hook-driven task tracking)
// ============================================================

/// Status of a task (one user prompt = one task).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task is currently being executed by the AI
    Active,
    /// Task completed normally
    Completed,
    /// Task has been fully rolled back
    RolledBack,
    /// Some changes in the task were rolled back
    PartialRolledBack,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Active => write!(f, "active"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::RolledBack => write!(f, "rolled-back"),
            TaskStatus::PartialRolledBack => write!(f, "partial-rolled-back"),
        }
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(TaskStatus::Active),
            "completed" => Ok(TaskStatus::Completed),
            "rolled-back" => Ok(TaskStatus::RolledBack),
            "partial-rolled-back" => Ok(TaskStatus::PartialRolledBack),
            _ => Err(format!("Unknown task status: {}", s)),
        }
    }
}

/// Risk level assessed for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::Low => write!(f, "low"),
            RiskLevel::Medium => write!(f, "medium"),
            RiskLevel::High => write!(f, "high"),
        }
    }
}

impl std::str::FromStr for RiskLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "low" => Ok(RiskLevel::Low),
            "medium" => Ok(RiskLevel::Medium),
            "high" => Ok(RiskLevel::High),
            _ => Err(format!("Unknown risk level: {}", s)),
        }
    }
}

/// Type of file change within a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeType::Created => write!(f, "created"),
            ChangeType::Modified => write!(f, "modified"),
            ChangeType::Deleted => write!(f, "deleted"),
            ChangeType::Renamed => write!(f, "renamed"),
        }
    }
}

impl std::str::FromStr for ChangeType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(ChangeType::Created),
            "modified" => Ok(ChangeType::Modified),
            "deleted" => Ok(ChangeType::Deleted),
            "renamed" => Ok(ChangeType::Renamed),
            _ => Err(format!("Unknown change type: {}", s)),
        }
    }
}

/// A task represents one user prompt → AI execution cycle.
///
/// This is the V2 core data model. Each task contains:
/// - The user's original prompt (intent)
/// - Which AI tool was used
/// - All file changes made during this task
/// - Status tracking for undo operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique identifier (nanoid-style short ID)
    pub id: String,
    /// User's original prompt text (from UserPromptSubmit hook)
    pub prompt: Option<String>,
    /// AI tool name (e.g. "claude-code", "cursor")
    pub tool: Option<String>,
    /// When the task started
    pub started_at: DateTime<Utc>,
    /// When the task completed (None if still active)
    pub completed_at: Option<DateTime<Utc>>,
    /// Current status
    pub status: TaskStatus,
    /// Risk level assessment
    pub risk_level: Option<RiskLevel>,
    /// AI-generated summary of changes
    pub summary: Option<String>,
    /// Project working directory (from AI tool hook's cwd field)
    pub cwd: Option<String>,
}

/// A single file change within a task.
///
/// Tracks what changed, with content hashes pointing to `.rew/objects/`
/// for content-addressable backup storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    /// Auto-increment ID
    pub id: Option<i64>,
    /// Which task this change belongs to
    pub task_id: String,
    /// Absolute path of the affected file
    pub file_path: PathBuf,
    /// Type of change
    pub change_type: ChangeType,
    /// SHA-256 hash of file content before change (in .rew/objects/)
    pub old_hash: Option<String>,
    /// SHA-256 hash of file content after change (in .rew/objects/)
    pub new_hash: Option<String>,
    /// Unified diff text (for text files only)
    pub diff_text: Option<String>,
    /// Lines added
    pub lines_added: u32,
    /// Lines removed
    pub lines_removed: u32,
    /// Set when the user individually restores this file (single-file rollback).
    /// NULL = not yet restored; non-NULL = timestamp of restoration.
    pub restored_at: Option<chrono::DateTime<chrono::Utc>>,
    /// How this change was attributed: "hook", "bash_predicted",
    /// "fsevent_active", "fsevent_grace", "monitoring", or "unknown".
    pub attribution: Option<String>,
    /// Original file path before rename (only set for Renamed changes).
    pub old_file_path: Option<PathBuf>,
}

/// Task with aggregated change statistics (returned by list queries).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskWithStats {
    pub task: Task,
    pub changes_count: u32,
    pub total_lines_added: u32,
    pub total_lines_removed: u32,
}

/// AI task performance statistics (one row per task).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStats {
    pub task_id: String,
    pub model: Option<String>,
    pub duration_secs: Option<f64>,
    pub tool_calls: i32,
    pub files_changed: i32,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub extra_json: Option<String>,
}
