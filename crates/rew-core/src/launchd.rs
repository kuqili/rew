//! LaunchAgent management for macOS auto-start.
//!
//! Generates and manages `~/Library/LaunchAgents/com.rew.agent.plist`
//! for automatic daemon startup on macOS login.

use crate::error::{RewError, RewResult};
use std::path::{Path, PathBuf};

/// LaunchAgent service identifier.
pub const LAUNCH_AGENT_LABEL: &str = "com.rew.agent";

/// Default plist filename.
pub const PLIST_FILENAME: &str = "com.rew.agent.plist";

/// Returns the LaunchAgents directory: ~/Library/LaunchAgents/
pub fn launch_agents_dir() -> PathBuf {
    let home = dirs::home_dir().expect("Could not determine home directory");
    home.join("Library").join("LaunchAgents")
}

/// Returns the full path to the rew plist file.
pub fn plist_path() -> PathBuf {
    launch_agents_dir().join(PLIST_FILENAME)
}

/// Generates the plist XML content for the LaunchAgent.
///
/// `executable_path`: Absolute path to the rew daemon binary.
/// `log_dir`: Directory for stdout/stderr logs (typically ~/.rew/).
pub fn generate_plist(executable_path: &Path, log_dir: &Path) -> String {
    let exe = executable_path.display();
    let stdout_log = log_dir.join("rew-daemon.stdout.log");
    let stderr_log = log_dir.join("rew-daemon.stderr.log");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>

    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>

    <key>ThrottleInterval</key>
    <integer>5</integer>

    <key>ProcessType</key>
    <string>Background</string>

    <key>StandardOutPath</key>
    <string>{stdout}</string>

    <key>StandardErrorPath</key>
    <string>{stderr}</string>

    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
</dict>
</plist>
"#,
        label = LAUNCH_AGENT_LABEL,
        exe = exe,
        stdout = stdout_log.display(),
        stderr = stderr_log.display(),
    )
}

/// Installs the LaunchAgent plist and loads it via launchctl.
///
/// Returns Ok(()) on success.
pub fn install(executable_path: &Path) -> RewResult<()> {
    let agents_dir = launch_agents_dir();
    std::fs::create_dir_all(&agents_dir)?;

    let log_dir = crate::rew_home_dir();
    std::fs::create_dir_all(&log_dir)?;

    let plist = plist_path();
    let content = generate_plist(executable_path, &log_dir);
    std::fs::write(&plist, &content)?;

    // Load the agent via launchctl
    let output = std::process::Command::new("launchctl")
        .args(["load", "-w"])
        .arg(&plist)
        .output()
        .map_err(|e| RewError::Io(e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // If already loaded, that's fine
        if !stderr.contains("already loaded") && !stderr.contains("service already loaded") {
            return Err(RewError::Config(format!(
                "launchctl load failed: {}",
                stderr.trim()
            )));
        }
    }

    Ok(())
}

/// Uninstalls the LaunchAgent: unload via launchctl and remove plist.
pub fn uninstall() -> RewResult<()> {
    let plist = plist_path();

    if plist.exists() {
        // Unload the agent
        let _ = std::process::Command::new("launchctl")
            .args(["unload", "-w"])
            .arg(&plist)
            .output();

        // Remove plist file
        std::fs::remove_file(&plist)?;
    }

    Ok(())
}

/// Checks if the LaunchAgent is currently installed (plist exists).
pub fn is_installed() -> bool {
    plist_path().exists()
}

/// Checks if the LaunchAgent is currently loaded and running.
pub fn is_running() -> bool {
    let output = std::process::Command::new("launchctl")
        .args(["list", LAUNCH_AGENT_LABEL])
        .output();

    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// Returns the PID of the running rew daemon, if any.
pub fn get_daemon_pid() -> Option<u32> {
    let output = std::process::Command::new("launchctl")
        .args(["list", LAUNCH_AGENT_LABEL])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // launchctl list <label> output format:
    // {
    //   "PID" = 12345;
    //   ...
    // }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with("\"PID\"") {
            // "PID" = 12345;
            if let Some(val) = line.split('=').nth(1) {
                let val = val.trim().trim_end_matches(';').trim();
                return val.parse::<u32>().ok();
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_generate_plist_contains_required_keys() {
        let exe = Path::new("/usr/local/bin/rew");
        let log_dir = Path::new("/Users/test/.rew");
        let plist = generate_plist(exe, log_dir);

        assert!(plist.contains("<string>com.rew.agent</string>"));
        assert!(plist.contains("<string>/usr/local/bin/rew</string>"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<true/>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>SuccessfulExit</key>"));
        assert!(plist.contains("<false/>"));
        assert!(plist.contains("<key>ThrottleInterval</key>"));
        assert!(plist.contains("<integer>5</integer>"));
        assert!(plist.contains("rew-daemon.stdout.log"));
        assert!(plist.contains("rew-daemon.stderr.log"));
    }

    #[test]
    fn test_plist_is_valid_xml() {
        let exe = Path::new("/Applications/rew.app/Contents/MacOS/rew");
        let log_dir = Path::new("/Users/test/.rew");
        let plist = generate_plist(exe, log_dir);

        assert!(plist.starts_with("<?xml version=\"1.0\""));
        assert!(plist.contains("<!DOCTYPE plist"));
        assert!(plist.contains("<plist version=\"1.0\">"));
        assert!(plist.ends_with("</plist>\n"));
    }

    #[test]
    fn test_launch_agents_dir_path() {
        let dir = launch_agents_dir();
        assert!(dir.ends_with("Library/LaunchAgents"));
    }

    #[test]
    fn test_plist_path() {
        let path = plist_path();
        assert!(path.ends_with("com.rew.agent.plist"));
    }

    #[test]
    fn test_keepalive_restarts_on_crash() {
        // Verify that KeepAlive is configured to restart on non-zero exit (crash)
        // SuccessfulExit = false means: restart if the process exits with non-zero
        let exe = Path::new("/usr/local/bin/rew");
        let log_dir = Path::new("/tmp");
        let plist = generate_plist(exe, log_dir);

        // KeepAlive with SuccessfulExit=false means:
        // - If process exits with status 0 (normal): do NOT restart
        // - If process exits with non-zero (crash/kill): DO restart
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>SuccessfulExit</key>"));
        assert!(plist.contains("<false/>"));
    }

    #[test]
    fn test_throttle_interval_prevents_rapid_restart() {
        let exe = Path::new("/usr/local/bin/rew");
        let log_dir = Path::new("/tmp");
        let plist = generate_plist(exe, log_dir);

        // ThrottleInterval = 5 seconds prevents restart loops
        assert!(plist.contains("<key>ThrottleInterval</key>"));
        assert!(plist.contains("<integer>5</integer>"));
    }
}
