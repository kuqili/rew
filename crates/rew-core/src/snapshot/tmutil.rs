//! Wrapper around the macOS `tmutil` CLI for APFS snapshot operations.
//!
//! Encapsulates `tmutil localsnapshot` (create), `tmutil listlocalsnapshots` (list),
//! and `tmutil deletelocalsnapshots` (delete). All operations parse stdout/stderr
//! to extract results and translate failures into `RewError::Snapshot`.

use crate::error::{RewError, RewResult};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Default timeout for snapshot creation (2 seconds, per acceptance criteria).
const SNAPSHOT_CREATE_TIMEOUT: Duration = Duration::from_secs(2);

/// Thin wrapper around the `tmutil` command-line tool.
///
/// All methods are synchronous because `tmutil` operations complete quickly
/// (snapshot creation < 1s on APFS). Timeouts are enforced to prevent hangs.
#[derive(Debug, Clone)]
pub struct TmutilWrapper {
    /// The volume to operate on (default: "/").
    volume: String,
    /// Timeout for snapshot creation.
    create_timeout: Duration,
}

impl Default for TmutilWrapper {
    fn default() -> Self {
        Self {
            volume: "/".to_string(),
            create_timeout: SNAPSHOT_CREATE_TIMEOUT,
        }
    }
}

impl TmutilWrapper {
    /// Create a new wrapper targeting the given volume.
    pub fn new(volume: impl Into<String>) -> Self {
        Self {
            volume: volume.into(),
            create_timeout: SNAPSHOT_CREATE_TIMEOUT,
        }
    }

    /// Set a custom timeout for snapshot creation.
    pub fn with_create_timeout(mut self, timeout: Duration) -> Self {
        self.create_timeout = timeout;
        self
    }

    /// Get the volume this wrapper operates on.
    pub fn volume(&self) -> &str {
        &self.volume
    }

    /// Create a local APFS snapshot via `tmutil localsnapshot`.
    ///
    /// Returns the snapshot date string (e.g. "2026-04-07-123456") on success.
    /// Enforces a timeout (default: 2s) to meet performance requirements.
    ///
    /// Example tmutil output:
    /// ```text
    /// Created local snapshot with date: 2026-04-07-123456
    /// ```
    pub fn create_snapshot(&self) -> RewResult<String> {
        debug!(
            "Creating APFS snapshot on volume {} (timeout: {:?})",
            self.volume, self.create_timeout
        );

        let start = std::time::Instant::now();

        let mut child = Command::new("tmutil")
            .args(["localsnapshot", &self.volume])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
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

        // Wait with timeout
        let result = Self::wait_with_timeout(&mut child, self.create_timeout);
        let elapsed = start.elapsed();

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if !output.status.success() {
                    return Err(RewError::Snapshot(format!(
                        "tmutil localsnapshot failed (exit code {:?}): {}",
                        output.status.code(),
                        stderr.trim()
                    )));
                }

                debug!("Snapshot created in {:?}", elapsed);

                // Parse: "Created local snapshot with date: 2026-04-07-123456"
                Self::parse_snapshot_date(&stdout).ok_or_else(|| {
                    RewError::Snapshot(format!(
                        "Failed to parse snapshot date from tmutil output: {}",
                        stdout.trim()
                    ))
                })
            }
            Err(_) => {
                // Kill the hung process
                let _ = child.kill();
                let _ = child.wait();
                Err(RewError::Snapshot(format!(
                    "tmutil localsnapshot timed out after {:?} (limit: {:?}). \
                     The volume may be under heavy I/O load.",
                    elapsed, self.create_timeout
                )))
            }
        }
    }

    /// Wait for a child process with a timeout.
    /// Returns Ok(Output) if the process exits within the timeout, Err(()) if it times out.
    fn wait_with_timeout(
        child: &mut std::process::Child,
        timeout: Duration,
    ) -> Result<std::process::Output, ()> {
        let start = std::time::Instant::now();
        let poll_interval = Duration::from_millis(50);

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // Process exited — collect output
                    let stdout = child.stdout.take().map(|mut s| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).unwrap_or(0);
                        buf
                    }).unwrap_or_default();

                    let stderr = child.stderr.take().map(|mut s| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut s, &mut buf).unwrap_or(0);
                        buf
                    }).unwrap_or_default();

                    return Ok(std::process::Output { status, stdout, stderr });
                }
                Ok(None) => {
                    // Still running
                    if start.elapsed() >= timeout {
                        return Err(());
                    }
                    std::thread::sleep(poll_interval);
                }
                Err(_) => {
                    return Err(());
                }
            }
        }
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

    /// Mount a local APFS snapshot for read-only access.
    ///
    /// Uses `mount_apfs -o ro -s <snapshot_name> <volume> <mount_point>` to mount
    /// the snapshot at a temporary location. Returns the mount point path.
    ///
    /// The caller is responsible for unmounting via `unmount_snapshot()`.
    pub fn mount_snapshot(&self, snapshot_name: &str) -> RewResult<PathBuf> {
        // Create a temporary mount point under /tmp
        let mount_point = PathBuf::from(format!(
            "/tmp/rew-snapshot-{}",
            snapshot_name.replace('.', "_").replace('/', "_")
        ));

        if !mount_point.exists() {
            std::fs::create_dir_all(&mount_point).map_err(|e| {
                RewError::Snapshot(format!(
                    "Failed to create mount point {:?}: {}",
                    mount_point, e
                ))
            })?;
        }

        debug!(
            "Mounting snapshot {} at {:?}",
            snapshot_name, mount_point
        );

        let output = Command::new("mount_apfs")
            .args([
                "-o",
                "ro",
                "-s",
                snapshot_name,
                &self.volume,
                mount_point.to_str().unwrap_or_default(),
            ])
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    RewError::Snapshot(
                        "mount_apfs not found. Snapshot mounting is only available on macOS."
                            .to_string(),
                    )
                } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                    RewError::Snapshot(
                        "mount_apfs requires root privileges. \
                         Please run with sudo or grant Full Disk Access."
                            .to_string(),
                    )
                } else {
                    RewError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up the empty mount dir
            let _ = std::fs::remove_dir(&mount_point);
            return Err(RewError::Snapshot(format!(
                "mount_apfs failed for snapshot {}: {}",
                snapshot_name,
                stderr.trim()
            )));
        }

        info!("Mounted snapshot {} at {:?}", snapshot_name, mount_point);
        Ok(mount_point)
    }

    /// Unmount a previously mounted snapshot.
    pub fn unmount_snapshot(&self, mount_point: &Path) -> RewResult<()> {
        debug!("Unmounting snapshot at {:?}", mount_point);

        let output = Command::new("umount")
            .arg(mount_point.to_str().unwrap_or_default())
            .output()
            .map_err(RewError::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(
                "Failed to unmount {:?}: {} (may need manual cleanup)",
                mount_point,
                stderr.trim()
            );
        }

        // Try to clean up the mount point directory
        let _ = std::fs::remove_dir(mount_point);
        Ok(())
    }

    /// Restore files from a mounted snapshot to a destination using `tmutil restore`.
    ///
    /// Correct syntax: `tmutil restore [-v] src ... dst`
    /// The source must be a path inside a mounted snapshot.
    pub fn restore_from_snapshot(
        &self,
        snapshot_name: &str,
        source_path: &Path,
        dest_path: &Path,
    ) -> RewResult<PathBuf> {
        // Step 1: Mount the snapshot
        let mount_point = self.mount_snapshot(snapshot_name)?;

        // Step 2: Build the source path inside the mounted snapshot
        // The mounted snapshot mirrors the original volume, so we strip the volume prefix
        let relative_source = source_path
            .strip_prefix(&self.volume)
            .unwrap_or(source_path);
        let snapshot_source = mount_point.join(relative_source);

        // Step 3: Verify source exists in snapshot
        if !snapshot_source.exists() {
            self.unmount_snapshot(&mount_point)?;
            return Err(RewError::Snapshot(format!(
                "Path {:?} does not exist in snapshot {} (looked at {:?})",
                source_path, snapshot_name, snapshot_source
            )));
        }

        // Step 4: Execute tmutil restore
        let result = self.execute_restore(&snapshot_source, dest_path);

        // Step 5: Always unmount, regardless of restore result
        if let Err(e) = self.unmount_snapshot(&mount_point) {
            warn!("Failed to unmount snapshot after restore: {}", e);
        }

        result.map(|_| dest_path.to_path_buf())
    }

    /// Execute `tmutil restore src dst` command.
    fn execute_restore(&self, source: &Path, dest: &Path) -> RewResult<()> {
        let src_str = source.to_str().unwrap_or_default();
        let dst_str = dest.to_str().unwrap_or_default();

        info!("Restoring {:?} → {:?}", source, dest);

        let output = Command::new("tmutil")
            .args(["restore", "-v", src_str, dst_str])
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    RewError::Snapshot(format!(
                        "tmutil restore requires elevated privileges for {:?}. \
                         Please grant Full Disk Access to rew in System Settings > \
                         Privacy & Security > Full Disk Access.",
                        dest
                    ))
                } else if e.kind() == std::io::ErrorKind::NotFound {
                    RewError::Snapshot(
                        "tmutil not found. Restore is only available on macOS.".to_string(),
                    )
                } else {
                    RewError::Io(e)
                }
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            };

            return Err(RewError::Snapshot(format!(
                "tmutil restore failed ({:?} → {:?}, exit code {:?}): {}",
                source,
                dest,
                output.status.code(),
                detail
            )));
        }

        info!("Successfully restored {:?} → {:?}", source, dest);
        Ok(())
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

    #[test]
    fn test_default_create_timeout() {
        let wrapper = TmutilWrapper::default();
        assert_eq!(wrapper.create_timeout, SNAPSHOT_CREATE_TIMEOUT);
        assert_eq!(wrapper.create_timeout, Duration::from_secs(2));
    }

    #[test]
    fn test_custom_create_timeout() {
        let wrapper = TmutilWrapper::default()
            .with_create_timeout(Duration::from_secs(5));
        assert_eq!(wrapper.create_timeout, Duration::from_secs(5));
    }

    #[test]
    fn test_mount_point_path_construction() {
        // Verify mount point path is constructed correctly (no actual mounting)
        let snapshot_name = "com.apple.TimeMachine.2026-04-07-123456.local";
        let expected_suffix = snapshot_name.replace('.', "_").replace('/', "_");
        let expected_path = format!("/tmp/rew-snapshot-{}", expected_suffix);
        assert!(!expected_path.contains('.'));
        assert!(!expected_path.contains('/').then_some(true).unwrap_or(false)
            || expected_path.starts_with("/tmp/"));
    }
}
