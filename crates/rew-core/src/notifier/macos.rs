//! macOS native notification support via osascript.
//!
//! Uses `osascript` to display notifications with action buttons.
//! Falls back to `terminal-notifier` if available.

use crate::error::{RewError, RewResult};
use crate::notifier::PlatformNotifier;
use crate::types::{AnomalySeverity, AnomalySignal};
use std::process::Command;
use tracing::{debug, error, info, warn};

/// macOS notification sender using osascript.
pub struct MacOSNotifier {
    /// App name shown in notifications
    app_name: String,
    /// Whether to use osascript (always true on macOS)
    enabled: bool,
}

impl MacOSNotifier {
    /// Create a new macOS notifier.
    pub fn new() -> Self {
        Self {
            app_name: "rew".to_string(),
            enabled: cfg!(target_os = "macos"),
        }
    }

    /// Create a notifier with a custom app name.
    pub fn with_app_name(app_name: &str) -> Self {
        Self {
            app_name: app_name.to_string(),
            enabled: cfg!(target_os = "macos"),
        }
    }

    /// Format the notification title based on severity.
    fn format_title(&self, signal: &AnomalySignal) -> String {
        let severity_icon = match signal.severity {
            AnomalySeverity::Critical => "\u{1F6A8}", // 🚨
            AnomalySeverity::High => "\u{26A0}\u{FE0F}",  // ⚠️
            AnomalySeverity::Medium => "\u{2139}\u{FE0F}", // ℹ️
        };
        format!(
            "{} [{}] {}",
            severity_icon,
            signal.severity,
            self.app_name
        )
    }

    /// Format the notification body for a single anomaly.
    fn format_body(&self, signal: &AnomalySignal) -> String {
        let dir = signal
            .primary_directory()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let file_count = signal.affected_files.len();

        match signal.severity {
            AnomalySeverity::Critical => {
                format!(
                    "CRITICAL: {}\nDirectory: {}\nImmediate snapshot created.",
                    signal.description, dir
                )
            }
            AnomalySeverity::High => {
                format!(
                    "{}\n{} file(s) affected in: {}",
                    signal.description, file_count, dir
                )
            }
            AnomalySeverity::Medium => {
                format!(
                    "{}\n{} file(s) in: {}",
                    signal.description, file_count, dir
                )
            }
        }
    }

    /// Format an aggregated summary for multiple MEDIUM alerts.
    fn format_aggregated_body(&self, signals: &[AnomalySignal]) -> String {
        let total_files: usize = signals.iter().map(|s| s.affected_files.len()).sum();
        let mut dirs: Vec<String> = signals
            .iter()
            .filter_map(|s| s.primary_directory().map(|d| d.display().to_string()))
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs.truncate(3);

        let mut body = format!(
            "{} alerts, {} total files affected\n",
            signals.len(),
            total_files
        );

        for (i, signal) in signals.iter().enumerate() {
            if i >= 5 {
                body.push_str(&format!("... and {} more\n", signals.len() - 5));
                break;
            }
            body.push_str(&format!("- {}\n", signal.kind_str()));
        }

        if !dirs.is_empty() {
            body.push_str(&format!("Directories: {}", dirs.join(", ")));
        }

        body
    }

    /// Send a notification via osascript.
    fn send_osascript(&self, title: &str, body: &str) -> RewResult<()> {
        if !self.enabled {
            info!("Notifications disabled (not macOS): {} — {}", title, body);
            return Ok(());
        }

        // Escape special characters for AppleScript string
        let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");

        let script = format!(
            "display notification \"{}\" with title \"{}\" sound name \"Blow\"",
            escaped_body, escaped_title
        );

        debug!("Sending osascript notification: {}", title);

        let result = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output();

        match result {
            Ok(output) => {
                if output.status.success() {
                    info!("Notification sent: {}", title);
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    error!("osascript failed: {}", stderr);
                    Err(RewError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("osascript failed: {}", stderr),
                    )))
                }
            }
            Err(e) => {
                warn!("Failed to run osascript: {}", e);
                Err(RewError::Io(e))
            }
        }
    }
}

impl Default for MacOSNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformNotifier for MacOSNotifier {
    fn send_anomaly(&self, signal: &AnomalySignal) -> RewResult<()> {
        let title = self.format_title(signal);
        let body = self.format_body(signal);
        self.send_osascript(&title, &body)
    }

    fn send_aggregated(&self, signals: &[AnomalySignal]) -> RewResult<()> {
        if signals.is_empty() {
            return Ok(());
        }
        let title = format!("\u{2139}\u{FE0F} [MEDIUM] {} — Summary", self.app_name);
        let body = self.format_aggregated_body(signals);
        self.send_osascript(&title, &body)
    }

    fn send_general(&self, title: &str, body: &str) -> RewResult<()> {
        self.send_osascript(title, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AnomalyKind, AnomalySeverity, AnomalySignal};
    use chrono::Utc;
    use std::path::PathBuf;

    fn make_signal(severity: AnomalySeverity, kind: AnomalyKind, dir: &str) -> AnomalySignal {
        let desc = format!("Test alert for {:?}", kind);
        AnomalySignal {
            kind,
            severity,
            affected_files: vec![
                PathBuf::from(format!("{}/file1.txt", dir)),
                PathBuf::from(format!("{}/file2.txt", dir)),
            ],
            detected_at: Utc::now(),
            description: desc,
        }
    }

    #[test]
    fn test_format_title_critical() {
        let notifier = MacOSNotifier::new();
        let signal = make_signal(
            AnomalySeverity::Critical,
            AnomalyKind::RootDirDeleted,
            "/home",
        );
        let title = notifier.format_title(&signal);
        assert!(title.contains("[CRITICAL]"));
        assert!(title.contains("rew"));
    }

    #[test]
    fn test_format_title_high() {
        let notifier = MacOSNotifier::new();
        let signal = make_signal(
            AnomalySeverity::High,
            AnomalyKind::BulkDelete,
            "/home/docs",
        );
        let title = notifier.format_title(&signal);
        assert!(title.contains("[HIGH]"));
    }

    #[test]
    fn test_format_body_includes_directory() {
        let notifier = MacOSNotifier::new();
        let signal = make_signal(
            AnomalySeverity::High,
            AnomalyKind::BulkDelete,
            "/home/user/Documents",
        );
        let body = notifier.format_body(&signal);
        assert!(body.contains("/home/user/Documents"));
        assert!(body.contains("2 file(s)"));
    }

    #[test]
    fn test_format_body_critical() {
        let notifier = MacOSNotifier::new();
        let signal = make_signal(
            AnomalySeverity::Critical,
            AnomalyKind::RootDirDeleted,
            "/home/user/Documents",
        );
        let body = notifier.format_body(&signal);
        assert!(body.contains("CRITICAL"));
        assert!(body.contains("snapshot"));
    }

    #[test]
    fn test_format_aggregated_body() {
        let notifier = MacOSNotifier::new();
        let signals = vec![
            make_signal(
                AnomalySeverity::Medium,
                AnomalyKind::BulkModify,
                "/home/proj1",
            ),
            make_signal(
                AnomalySeverity::Medium,
                AnomalyKind::LargeNonPackageModify,
                "/home/proj2",
            ),
        ];
        let body = notifier.format_aggregated_body(&signals);
        assert!(body.contains("2 alerts"));
        assert!(body.contains("4 total files"));
        assert!(body.contains("BulkModify"));
        assert!(body.contains("LargeNonPackageModify"));
    }

    #[test]
    fn test_format_aggregated_body_truncation() {
        let notifier = MacOSNotifier::new();
        let signals: Vec<AnomalySignal> = (0..8)
            .map(|i| {
                make_signal(
                    AnomalySeverity::Medium,
                    AnomalyKind::BulkModify,
                    &format!("/home/proj{}", i),
                )
            })
            .collect();
        let body = notifier.format_aggregated_body(&signals);
        assert!(body.contains("... and 3 more"));
    }

    #[test]
    fn test_custom_app_name() {
        let notifier = MacOSNotifier::with_app_name("RewGuard");
        let signal = make_signal(
            AnomalySeverity::High,
            AnomalyKind::BulkDelete,
            "/home",
        );
        let title = notifier.format_title(&signal);
        assert!(title.contains("RewGuard"));
    }

    #[test]
    fn test_sensitive_file_body() {
        let notifier = MacOSNotifier::new();
        let signal = AnomalySignal {
            kind: AnomalyKind::SensitiveConfigModified,
            severity: AnomalySeverity::High,
            affected_files: vec![PathBuf::from("/project/.env")],
            detected_at: Utc::now(),
            description: "RULE-06: Sensitive config file(s) modified: /project/.env".to_string(),
        };
        let body = notifier.format_body(&signal);
        assert!(body.contains(".env"));
        assert!(body.contains("/project"));
    }
}
