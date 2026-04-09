//! `rew install` / `rew uninstall` — LaunchAgent + AI tool hook management.

use crate::display;
use rew_core::error::RewResult;
use rew_core::launchd;
use std::path::PathBuf;

/// Install the LaunchAgent AND inject hooks into detected AI tools.
pub fn install() -> RewResult<()> {
    // Find the rew binary path
    let exe = std::env::current_exe().map_err(|e| {
        rew_core::error::RewError::Config(format!("Could not determine rew binary path: {}", e))
    })?;

    // 1. Install LaunchAgent
    println!(
        "{} 正在安装 LaunchAgent...",
        display::info_prefix()
    );

    launchd::install(&exe)?;

    println!(
        "{} LaunchAgent 已安装",
        display::success_prefix()
    );

    // 2. Detect and inject hooks for AI tools
    println!();
    println!(
        "{} 正在检测 AI 工具并注入 hook...",
        display::info_prefix()
    );

    let rew_bin = exe.to_string_lossy().to_string();
    let mut installed_count = 0;

    // Claude Code
    if let Some(result) = inject_claude_code_hooks(&rew_bin) {
        match result {
            Ok(()) => {
                println!(
                    "{} Claude Code hook 已注入",
                    display::success_prefix()
                );
                installed_count += 1;
            }
            Err(e) => {
                println!("  ⚠ Claude Code hook 注入失败: {}", e);
            }
        }
    }

    // Cursor
    if let Some(result) = inject_cursor_hooks(&rew_bin) {
        match result {
            Ok(()) => {
                println!(
                    "{} Cursor hook 已注入",
                    display::success_prefix()
                );
                installed_count += 1;
            }
            Err(e) => {
                println!("  ⚠ Cursor hook 注入失败: {}", e);
            }
        }
    }

    if installed_count == 0 {
        println!("  未检测到已安装的 AI 工具，将使用纯文件监听模式");
    }

    // 3. Generate default .rewscope if not exists
    let cwd = std::env::current_dir().unwrap_or_default();
    let scope_file = cwd.join(".rewscope");
    if !scope_file.exists() {
        let default_scope = r#"# rew scope rules
# AI tools can only operate within the allowed scope.
# Paths matching 'deny' will be blocked (PreToolUse exit 2).

allow:
  - "./**"

deny:
  - "~/Desktop/**"
  - "~/Documents/**"
  - "~/Downloads/**"
  - "~/.ssh/**"
  - "~/.aws/**"
  - "/**/.env"
  - "/**/.env.*"

alert:
  - pattern: "rm -rf"
    action: block
  - pattern: "> /dev/"
    action: block
"#;
        if std::fs::write(&scope_file, default_scope).is_ok() {
            println!(
                "{} 已生成 .rewscope 规则文件",
                display::success_prefix()
            );
        }
    }

    println!();
    println!("  使用 {} 立即启动守护进程", colored::Colorize::cyan("rew daemon"));

    Ok(())
}

/// Uninstall the LaunchAgent AND remove hooks from AI tools.
pub fn uninstall() -> RewResult<()> {
    // 1. Remove hooks
    let rew_bin = std::env::current_exe()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();

    remove_claude_code_hooks(&rew_bin);
    remove_cursor_hooks(&rew_bin);

    // 2. Remove LaunchAgent
    if launchd::is_installed() {
        println!(
            "{} 正在卸载 LaunchAgent...",
            display::info_prefix()
        );

        launchd::uninstall()?;

        println!(
            "{} LaunchAgent 已卸载",
            display::success_prefix()
        );
    } else {
        println!(
            "{} LaunchAgent 未安装，无需卸载",
            display::info_prefix()
        );
    }

    println!("  所有 rew hook 已清理");

    Ok(())
}

// ================================================================
// Claude Code hook injection
// ================================================================

/// Detect Claude Code and inject hooks. Returns None if not installed.
fn inject_claude_code_hooks(rew_bin: &str) -> Option<Result<(), String>> {
    let home = dirs::home_dir()?;

    // Claude Code uses ~/.claude/settings.json (or ~/.claude-internal/settings.json)
    let settings_paths = vec![
        home.join(".claude").join("settings.json"),
        home.join(".claude-internal").join("settings.json"),
    ];

    for settings_path in &settings_paths {
        if settings_path.parent().map(|p| p.exists()).unwrap_or(false) {
            return Some(inject_claude_code_hooks_to(settings_path, rew_bin));
        }
    }

    None
}

fn inject_claude_code_hooks_to(settings_path: &PathBuf, rew_bin: &str) -> Result<(), String> {
    // Read existing settings or create empty
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(settings_path)
            .map_err(|e| format!("读取失败: {}", e))?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Build hooks configuration
    // Claude Code hook format: hooks.PreToolUse = [{ "matcher": "...", "hooks": [...] }]
    let hook_entry = serde_json::json!({
        "type": "command",
        "command": format!("{} hook pre-tool", rew_bin)
    });

    let prompt_hook = serde_json::json!({
        "type": "command",
        "command": format!("{} hook prompt", rew_bin)
    });

    let post_hook = serde_json::json!({
        "type": "command",
        "command": format!("{} hook post-tool", rew_bin)
    });

    let stop_hook = serde_json::json!({
        "type": "command",
        "command": format!("{} hook stop", rew_bin)
    });

    let hooks = settings.as_object_mut()
        .ok_or("settings.json 不是有效的 JSON 对象")?;

    // Ensure hooks section exists
    if !hooks.contains_key("hooks") {
        hooks.insert("hooks".to_string(), serde_json::json!({}));
    }

    let hooks_section = hooks.get_mut("hooks").unwrap().as_object_mut()
        .ok_or("hooks 不是有效的 JSON 对象")?;

    // Set PreToolUse hook
    hooks_section.insert("PreToolUse".to_string(), serde_json::json!([{
        "matcher": "",
        "hooks": [hook_entry]
    }]));

    // Set UserPromptSubmit hook
    hooks_section.insert("UserPromptSubmit".to_string(), serde_json::json!([{
        "matcher": "",
        "hooks": [prompt_hook]
    }]));

    // Set PostToolUse hook
    hooks_section.insert("PostToolUse".to_string(), serde_json::json!([{
        "matcher": "",
        "hooks": [post_hook]
    }]));

    // Set Stop hook
    hooks_section.insert("Stop".to_string(), serde_json::json!([{
        "matcher": "",
        "hooks": [stop_hook]
    }]));

    // Write back
    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("JSON 序列化失败: {}", e))?;
    std::fs::write(settings_path, content)
        .map_err(|e| format!("写入失败: {}", e))?;

    Ok(())
}

fn remove_claude_code_hooks(rew_bin: &str) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };

    let settings_paths = vec![
        home.join(".claude").join("settings.json"),
        home.join(".claude-internal").join("settings.json"),
    ];

    for path in &settings_paths {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if content.contains(rew_bin) || content.contains("rew hook") {
                    // Remove rew hooks from settings
                    if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                            hooks.remove("PreToolUse");
                            hooks.remove("UserPromptSubmit");
                            hooks.remove("PostToolUse");
                            hooks.remove("Stop");
                        }
                        if let Ok(new_content) = serde_json::to_string_pretty(&settings) {
                            let _ = std::fs::write(path, new_content);
                        }
                    }
                }
            }
        }
    }
}

// ================================================================
// Cursor hook injection
// ================================================================

fn inject_cursor_hooks(rew_bin: &str) -> Option<Result<(), String>> {
    let home = dirs::home_dir()?;
    let cursor_dir = home.join(".cursor");

    if !cursor_dir.exists() {
        return None;
    }

    let hooks_path = cursor_dir.join("hooks.json");
    Some(inject_cursor_hooks_to(&hooks_path, rew_bin))
}

fn inject_cursor_hooks_to(hooks_path: &PathBuf, rew_bin: &str) -> Result<(), String> {
    let hooks = serde_json::json!({
        "beforeShellExecution": [{
            "command": format!("{} hook pre-tool", rew_bin),
            "description": "rew: scope check before shell execution"
        }],
        "afterFileEdit": [{
            "command": format!("{} hook post-tool", rew_bin),
            "description": "rew: record file change"
        }],
        "beforeSubmitPrompt": [{
            "command": format!("{} hook prompt", rew_bin),
            "description": "rew: create task on prompt submit"
        }],
        "stop": [{
            "command": format!("{} hook stop", rew_bin),
            "description": "rew: close task"
        }]
    });

    let content = serde_json::to_string_pretty(&hooks)
        .map_err(|e| format!("JSON 序列化失败: {}", e))?;
    std::fs::write(hooks_path, content)
        .map_err(|e| format!("写入失败: {}", e))?;

    Ok(())
}

fn remove_cursor_hooks(_rew_bin: &str) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };

    let hooks_path = home.join(".cursor").join("hooks.json");
    if hooks_path.exists() {
        // Simply remove the hooks file (Cursor will ignore missing file)
        let _ = std::fs::remove_file(hooks_path);
    }
}
