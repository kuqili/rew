//! Wrapper around the macOS `tmutil` CLI for APFS snapshot operations.
//!
//! Encapsulates `tmutil localsnapshot` (create), `tmutil listlocalsnapshots` (list),
//! and `tmutil deletelocalsnapshots` (delete). All operations parse stdout/stderr
//! to extract results and translate failures into `RewError::Snapshot`.

use crate::error::{RewError, RewResult};
use std::process::Command;
use tracing::{debug, warn};

/// Thin wrapper around the `tmutil` command-line tool.
///
/// All methods are synchronous because `tmutil` operations complete quickly
/// (snapshot creation < 1s on APFS).
#[derive(Debug, Clone)]
pub struct TmutilWrapper {
    /// The volume to operate on (default: "/").
    volume: String,
}

impl Default for TmutilWrapper {
    fn default() -> Self {
        Self {
            volume: "/".to_string(),
        }
    }
}

impl TmutilWrapper {
    /// Create a new wrapper targeting the given volume.
    pub fn new(volume: impl Into<String>) -> Self {
        Self {
            volume: volume.into(),
        }
    }

    /// Create a local APFS snapshot via `tmutil localsnapshot`.
    ///
    /// Returns the snapshot date string (e.g. "2026-04-07-123456") on success.
    ///
    /// Example tmutil output:
    /// ```text
    /// Created local snapshot with date: 2026-04-07-123456
    /// ```
    pub fn create_snapshot(&self) -> RewResult<String> {
        debug!("Creating APFS snapshot on volume {}", self.volume);

        let output = Command::new("tmutil")
            .args(["localsnapshot", &self.volume])
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    RewError::Snapshot(
                        "tmutil requires elevated privileges. Please grant Full Disk Access \
                         to rew in System Settings > Privacy & Security > Full Disk Access."
                            .to_string(),
                    )
                } else if e.kind() == std::io::ErrorKind::NotFound {
                    RewError::Snapshot(
                        "tmutil not found. APFS snapshots are only available on macOS."
                            .to_string(),
                    )
                } else {
                    RewError::Io(e)
                }
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(RewError::Snapshot(format!(
                "tmutil localsnapshot failed (exit code {:?}): {}",
                output.status.code(),
                stderr.trim()
            )));
        }

        // Parse: "Created local snapshot with date: 2026-04-07-123456"
        Self::parse_snapshot_date(&stdout).ok_or_else(|| {
            RewError::Snapshot(format!(
                "Failed to parse snapshot date from tmutil output: {}",
                stdout.trim()
            ))
        })
    }

    /// List all local APFS snapshots via `tmutil listlocalsnapshots`.
    ///
    /// Returns snapshot name strings (e.g. "com.apple.TimeMachine.2026-04-07-123456.local").
    ///
    /// Example tmutil output:
    /// ```text
    /// Snapshots for disk /:
    /// com.apple.TimeMachine.2026-04-07-120000.local
    /// com.apple.TimeMachine.2026-04-07-130000.local
    /// ```
    pub fn list_snapshots(&self) -> RewResult<Vec<String>> {
        debug!("Listing APFS snapshots on volume {}", self.volume);

        let output = Command::new("tmutil")
            .args(["listlocalsnapshots", &self.volume])
            .output()
            .map_err(|e| RewError::Io(e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RewError::Snapshot(format!(
                "tmutil listlocalsnapshots failed: {}",
                stderr.trim()
            )));
        }

        Ok(Self::parse_snapshot_list(&stdout))
    }

    /// Delete a local APFS snapshot by its date string via `tmutil deletelocalsnapshots`.
    ///
    /// The `snapshot_date` should be the date portion, e.g. "2026-04-07-123456".
    pub fn delete_snapshot(&self, snapshot_date: &str) -> RewResult<()> {
        debug!("Deleting APFS snapshot: {}", snapshot_date);

        let output = Command::new("tmutil")
            .args(["deletelocalsnapshots", snapshot_date])
            .output()
            .map_err(|e| RewError::Io(e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let msg = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            };
            // If the snapshot was already deleted by the OS, treat it as a warning, not error
            if msg.contains("Could not delete")
                || msg.contains("No matching snapshot")
                || msg.contains("not found")
            {
                warn!(
                    "Snapshot {} may have been deleted by the OS: {}",
                    snapshot_date, msg
                );
                return Ok(());
            }
            return Err(RewError::Snapshot(format!(
                "tmutil deletelocalsnapshots failed for {}: {}",
                snapshot_date, msg
            )));
        }

        debug!("Successfully deleted snapshot: {}", snapshot_date);
        Ok(())
    }

    /// Check if tmutil is available and working.
    pub fn is_available(&self) -> bool {
        Command::new("tmutil")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    // -- Parsing helpers --

    /// Parse the snapshot date from `tmutil localsnapshot` output.
    /// Input: "Created local snapshot with date: 2026-04-07-123456\n"
    /// Output: Some("2026-04-07-123456")
    fn parse_snapshot_date(output: &str) -> Option<String> {
        for line in output.lines() {
            let trimmed = line.trim();
            if let Some(pos) = trimmed.find("date:") {
                let date_part = trimmed[pos + 5..].trim();
                if !date_part.is_empty() {
                    return Some(date_part.to_string());
                }
            }
        }
        None
    }

    /// Parse the snapshot list from `tmutil listlocalsnapshots` output.
    /// Skips the header line ("Snapshots for disk /:" or "Snapshots for volume group:").
    /// Returns full snapshot names like "com.apple.TimeMachine.2026-04-07-123456.local".
    fn parse_snapshot_list(output: &str) -> Vec<String> {
        output
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with("Snapshots for"))
            .map(|l| l.to_string())
            .collect()
    }

    /// Extract the date portion from a full snapshot name.
    /// Input: "com.apple.TimeMachine.2026-04-07-123456.local"
    /// Output: Some("2026-04-07-123456")
    pub fn extract_date_from_name(snapshot_name: &str) -> Option<String> {
        // Format: com.apple.TimeMachine.YYYY-MM-DD-HHMMSS.local
        let stripped = snapshot_name
            .strip_prefix("com.apple.TimeMachine.")
            .unwrap_or(snapshot_name);
        let stripped = stripped.strip_suffix(".local").unwrap_or(stripped);
        // Validate it looks like a date: YYYY-MM-DD-HHMMSS
        if stripped.len() >= 17
            && stripped.chars().nth(4) == Some('-')
            && stripped.chars().nth(7) == Some('-')
            && stripped.chars().nth(10) == Some('-')
        {
            Some(stripped.to_string())
        } else {
            // Return as-is if format doesn't match (could be custom prefix)
            Some(stripped.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snapshot_date() {
        let output = "Created local snapshot with date: 2026-04-07-123456\n";
        assert_eq!(
            TmutilWrapper::parse_snapshot_date(output),
            Some("2026-04-07-123456".to_string())
        );
    }

    #[test]
    fn test_parse_snapshot_date_no_match() {
        let output = "Some other output\n";
        assert_eq!(TmutilWrapper::parse_snapshot_date(output), None);
    }

    #[test]
    fn test_parse_snapshot_list() {
        let output = "\
Snapshots for disk /:
com.apple.TimeMachine.2026-04-07-120000.local
com.apple.TimeMachine.2026-04-07-130000.local
com.apple.TimeMachine.2026-04-07-140000.local
";
        let result = TmutilWrapper::parse_snapshot_list(output);
        assert_eq!(result.len(), 3);
        assert_eq!(
            result[0],
            "com.apple.TimeMachine.2026-04-07-120000.local"
        );
    }

    #[test]
    fn test_parse_snapshot_list_empty() {
        let output = "Snapshots for disk /:\n";
        let result = TmutilWrapper::parse_snapshot_list(output);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_snapshot_list_volume_group() {
        let output = "\
Snapshots for volume group containing disk /:
com.apple.TimeMachine.2026-04-07-120000.local
";
        let result = TmutilWrapper::parse_snapshot_list(output);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_extract_date_from_name() {
        let name = "com.apple.TimeMachine.2026-04-07-123456.local";
        assert_eq!(
            TmutilWrapper::extract_date_from_name(name),
            Some("2026-04-07-123456".to_string())
        );
    }

    #[test]
    fn test_extract_date_from_name_no_prefix() {
        let name = "2026-04-07-123456";
        assert_eq!(
            TmutilWrapper::extract_date_from_name(name),
            Some("2026-04-07-123456".to_string())
        );
    }
}
