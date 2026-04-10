//! `rew install` / `rew uninstall` — AI tool hook management.

use crate::display;
use rew_core::error::RewResult;
use std::path::PathBuf;

/// Detect AI tools and inject hooks.
pub fn install() -> RewResult<()> {
    let exe = std::env::current_exe().map_err(|e| {
        rew_core::error::RewError::Config(format!("Could not determine rew binary path: {}", e))
    })?;

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
  - "~/.ssh/**"
  - "~/.aws/**"
  - "~/.rew/**"
  - "/**/.env"
  - "/**/.env.*"
"#;
        if std::fs::write(&scope_file, default_scope).is_ok() {
            println!(
                "{} 已生成 .rewscope 规则文件",
                display::success_prefix()
            );
        }
    }

    Ok(())
}

/// Remove hooks from AI tools.
pub fn uninstall() -> RewResult<()> {
    let rew_bin = std::env::current_exe()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();

    remove_claude_code_hooks(&rew_bin);
    remove_cursor_hooks(&rew_bin);

    println!(
        "{} 所有 rew hook 已清理",
        display::success_prefix()
    );

    Ok(())
}

// ================================================================
// Claude Code hook injection
// ================================================================

/// Detect Claude Code and inject hooks. Returns None if not installed.
fn inject_claude_code_hooks(rew_bin: &str) -> Option<Result<(), String>> {
    let home = dirs::home_dir()?;

    // Claude Code uses ~/.claude/settings.json (or ~/.claude-internal/settings.json)
    // Prefer ~/.claude-internal/ first (Tencent internal build takes precedence)
    let settings_paths = vec![
        home.join(".claude-internal").join("settings.json"),
        home.join(".claude").join("settings.json"),
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
    // Claude Code hook format: hooks.EventName = [{ "matcher": "...", "hooks": [...] }]
    let pre_tool_hook = serde_json::json!({
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

    // Helper: add rew hook entry to an event, preserving existing non-rew hooks
    fn add_rew_hook(
        hooks_section: &mut serde_json::Map<String, serde_json::Value>,
        event_name: &str,
        matcher: &str,
        hook_entry: serde_json::Value,
    ) {
        // First remove any existing rew entries for this event
        if let Some(existing) = hooks_section.get(event_name).and_then(|v| v.as_array()) {
            let filtered: Vec<serde_json::Value> = existing.iter()
                .filter(|entry| {
                    // Keep entries that don't contain "rew hook" in any of their hook commands
                    if let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) {
                        !hooks.iter().any(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains("rew hook"))
                                .unwrap_or(false)
                        })
                    } else {
                        true
                    }
                })
                .cloned()
                .collect();
            hooks_section.insert(event_name.to_string(), serde_json::Value::Array(filtered));
        }

        // Now append the new rew entry
        let rew_entry = serde_json::json!({
            "matcher": matcher,
            "hooks": [hook_entry]
        });

        if let Some(arr) = hooks_section.get_mut(event_name).and_then(|v| v.as_array_mut()) {
            arr.push(rew_entry);
        } else {
            hooks_section.insert(event_name.to_string(), serde_json::json!([rew_entry]));
        }
    }

    // Set PreToolUse hook (match Write/Edit/Bash tools only)
    add_rew_hook(hooks_section, "PreToolUse", "Write|Edit|MultiEdit|Bash", pre_tool_hook);

    // Set UserPromptSubmit hook
    add_rew_hook(hooks_section, "UserPromptSubmit", "", prompt_hook);

    // Set PostToolUse hook (match Write/Edit/Bash tools only)
    add_rew_hook(hooks_section, "PostToolUse", "Write|Edit|MultiEdit|Bash", post_hook);

    // Set Stop hook
    add_rew_hook(hooks_section, "Stop", "", stop_hook);

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
        home.join(".claude-internal").join("settings.json"),
        home.join(".claude").join("settings.json"),
    ];

    for path in &settings_paths {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if content.contains(rew_bin) || content.contains("rew hook") {
                    if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                            // Only remove rew-related entries from each event, preserve others
                            for (_event_name, entries) in hooks.iter_mut() {
                                if let Some(arr) = entries.as_array_mut() {
                                    arr.retain(|entry| {
                                        if let Some(hook_list) = entry.get("hooks").and_then(|h| h.as_array()) {
                                            !hook_list.iter().any(|h| {
                                                h.get("command")
                                                    .and_then(|c| c.as_str())
                                                    .map(|c| c.contains("rew hook"))
                                                    .unwrap_or(false)
                                            })
                                        } else {
                                            true // Keep entries we don't understand
                                        }
                                    });
                                }
                            }

                            // Clean up empty event arrays
                            let empty_events: Vec<String> = hooks.iter()
                                .filter(|(_, v)| v.as_array().map(|a| a.is_empty()).unwrap_or(false))
                                .map(|(k, _)| k.clone())
                                .collect();
                            for key in empty_events {
                                hooks.remove(&key);
                            }
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
