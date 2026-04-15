//! AI tool hook management — detect, install, and uninstall rew hooks for
//! supported AI tools (Claude Code, Cursor, CodeBuddy, WorkBuddy).
//!
//! This module lives in `rew-core` so both the Tauri GUI and the CLI can
//! share the same logic without duplication.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Represents a detected AI tool and its current hook status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiToolInfo {
    /// Machine-readable identifier, e.g. "claude_code", "cursor"
    pub id: String,
    /// Human-readable display name
    pub name: String,
    /// Whether rew hooks are currently installed for this tool
    pub hook_installed: bool,
    /// Path to the configuration file that holds hooks
    pub config_path: Option<String>,
}

/// Detect all supported AI tools installed on this machine and return their
/// hook status.
pub fn detect_ai_tools(rew_bin: &str) -> Vec<AiToolInfo> {
    let mut tools = Vec::new();

    if let Some(info) = detect_claude_code(rew_bin) {
        tools.push(info);
    }
    if let Some(info) = detect_cursor(rew_bin) {
        tools.push(info);
    }
    if let Some(info) = detect_codebuddy(rew_bin) {
        tools.push(info);
    }
    if let Some(info) = detect_workbuddy(rew_bin) {
        tools.push(info);
    }

    tools
}

/// Install rew hooks for a specific tool. Returns Ok(()) on success.
pub fn install_hook(tool_id: &str, rew_bin: &str) -> Result<(), String> {
    match tool_id {
        "claude_code" => install_claude_code_hooks(rew_bin),
        "cursor" => install_cursor_hooks(rew_bin),
        "codebuddy" => install_codebuddy_hooks(rew_bin),
        "workbuddy" => install_workbuddy_hooks(rew_bin),
        _ => Err(format!("不支持的工具: {}", tool_id)),
    }
}

/// Uninstall rew hooks for a specific tool.
pub fn uninstall_hook(tool_id: &str, rew_bin: &str) -> Result<(), String> {
    match tool_id {
        "claude_code" => remove_claude_code_hooks(rew_bin),
        "cursor" => remove_cursor_hooks(rew_bin),
        "codebuddy" => remove_codebuddy_hooks(rew_bin),
        "workbuddy" => remove_workbuddy_hooks(rew_bin),
        _ => Err(format!("不支持的工具: {}", tool_id)),
    }
}

// ================================================================
// Claude Code
// ================================================================

fn claude_code_settings_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidates = [
        home.join(".claude-internal").join("settings.json"),
        home.join(".claude").join("settings.json"),
    ];
    for path in &candidates {
        if path.parent().map(|p| p.exists()).unwrap_or(false) {
            return Some(path.clone());
        }
    }
    None
}

fn detect_claude_code(rew_bin: &str) -> Option<AiToolInfo> {
    let settings_path = claude_code_settings_path()?;
    let hook_installed = check_claude_code_hooks(&settings_path, rew_bin);
    Some(AiToolInfo {
        id: "claude_code".to_string(),
        name: "Claude Code".to_string(),
        hook_installed,
        config_path: Some(settings_path.to_string_lossy().to_string()),
    })
}

fn check_claude_code_hooks(settings_path: &PathBuf, _rew_bin: &str) -> bool {
    if !settings_path.exists() {
        return false;
    }
    let content = match std::fs::read_to_string(settings_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    content.contains("rew hook")
}

fn install_claude_code_hooks(rew_bin: &str) -> Result<(), String> {
    let settings_path = claude_code_settings_path()
        .ok_or("未检测到 Claude Code 安装")?;

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("读取失败: {}", e))?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let pre_tool_hook = serde_json::json!({
        "type": "command",
        "command": format!("{} hook pre-tool --source claude-code", rew_bin)
    });
    let prompt_hook = serde_json::json!({
        "type": "command",
        "command": format!("{} hook prompt --source claude-code", rew_bin)
    });
    let post_hook = serde_json::json!({
        "type": "command",
        "command": format!("{} hook post-tool --source claude-code", rew_bin)
    });
    let stop_hook = serde_json::json!({
        "type": "command",
        "command": format!("{} hook stop --source claude-code", rew_bin)
    });

    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json 不是有效的 JSON 对象")?;

    if !hooks.contains_key("hooks") {
        hooks.insert("hooks".to_string(), serde_json::json!({}));
    }

    let hooks_section = hooks
        .get_mut("hooks")
        .unwrap()
        .as_object_mut()
        .ok_or("hooks 不是有效的 JSON 对象")?;

    add_rew_hook(hooks_section, "PreToolUse", "Write|Edit|MultiEdit|Bash", pre_tool_hook);
    add_rew_hook(hooks_section, "UserPromptSubmit", "", prompt_hook);
    add_rew_hook(hooks_section, "PostToolUse", "Write|Edit|MultiEdit|Bash", post_hook);
    add_rew_hook(hooks_section, "Stop", "", stop_hook);

    // Ensure parent directory exists
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {}", e))?;
    }

    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("JSON 序列化失败: {}", e))?;
    std::fs::write(&settings_path, content)
        .map_err(|e| format!("写入失败: {}", e))?;

    Ok(())
}

fn remove_claude_code_hooks(_rew_bin: &str) -> Result<(), String> {
    let settings_path = claude_code_settings_path()
        .ok_or("未检测到 Claude Code 安装")?;

    if !settings_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&settings_path)
        .map_err(|e| format!("读取失败: {}", e))?;

    if !content.contains("rew hook") {
        return Ok(());
    }

    let mut settings: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("JSON 解析失败: {}", e))?;

    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
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
                        true
                    }
                });
            }
        }

        let empty_events: Vec<String> = hooks
            .iter()
            .filter(|(_, v)| v.as_array().map(|a| a.is_empty()).unwrap_or(false))
            .map(|(k, _)| k.clone())
            .collect();
        for key in empty_events {
            hooks.remove(&key);
        }
    }

    let new_content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("JSON 序列化失败: {}", e))?;
    std::fs::write(&settings_path, new_content)
        .map_err(|e| format!("写入失败: {}", e))?;

    Ok(())
}

/// Helper: add rew hook entry to a Claude Code event, preserving non-rew hooks.
fn add_rew_hook(
    hooks_section: &mut serde_json::Map<String, serde_json::Value>,
    event_name: &str,
    matcher: &str,
    hook_entry: serde_json::Value,
) {
    if let Some(existing) = hooks_section.get(event_name).and_then(|v| v.as_array()) {
        let filtered: Vec<serde_json::Value> = existing
            .iter()
            .filter(|entry| {
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

    let rew_entry = serde_json::json!({
        "matcher": matcher,
        "hooks": [hook_entry]
    });

    if let Some(arr) = hooks_section
        .get_mut(event_name)
        .and_then(|v| v.as_array_mut())
    {
        arr.push(rew_entry);
    } else {
        hooks_section.insert(event_name.to_string(), serde_json::json!([rew_entry]));
    }
}

// ================================================================
// Cursor
// ================================================================

fn cursor_hooks_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let cursor_dir = home.join(".cursor");
    if cursor_dir.exists() {
        Some(cursor_dir.join("hooks.json"))
    } else {
        None
    }
}

fn detect_cursor(rew_bin: &str) -> Option<AiToolInfo> {
    let home = dirs::home_dir()?;
    let cursor_dir = home.join(".cursor");
    if !cursor_dir.exists() {
        return None;
    }
    let hooks_path = cursor_dir.join("hooks.json");
    let hook_installed = check_cursor_hooks(&hooks_path, rew_bin);
    Some(AiToolInfo {
        id: "cursor".to_string(),
        name: "Cursor".to_string(),
        hook_installed,
        config_path: Some(hooks_path.to_string_lossy().to_string()),
    })
}

fn check_cursor_hooks(hooks_path: &PathBuf, _rew_bin: &str) -> bool {
    if !hooks_path.exists() {
        return false;
    }
    let content = match std::fs::read_to_string(hooks_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    content.contains("rew hook")
}

/// Read or create the Cursor hooks.json with proper `{ "version": 1, "hooks": { ... } }` structure.
fn read_cursor_hooks(hooks_path: &PathBuf) -> Result<serde_json::Value, String> {
    if hooks_path.exists() {
        let content = std::fs::read_to_string(hooks_path)
            .map_err(|e| format!("读取失败: {}", e))?;
        let val: serde_json::Value = serde_json::from_str(&content)
            .unwrap_or(serde_json::json!({}));

        // Migrate old format (events at top level) to new format (nested under "hooks")
        if val.get("hooks").is_none() && val.is_object() {
            let mut migrated = serde_json::json!({ "version": 1, "hooks": {} });
            if let Some(old_obj) = val.as_object() {
                let hooks_obj = migrated.get_mut("hooks").unwrap().as_object_mut().unwrap();
                for (k, v) in old_obj {
                    if k == "version" {
                        continue;
                    }
                    hooks_obj.insert(k.clone(), v.clone());
                }
            }
            return Ok(migrated);
        }

        // Ensure version field exists
        if val.get("version").is_none() {
            let mut v = val;
            v.as_object_mut()
                .map(|o| o.insert("version".to_string(), serde_json::json!(1)));
            return Ok(v);
        }

        Ok(val)
    } else {
        Ok(serde_json::json!({ "version": 1, "hooks": {} }))
    }
}

/// Helper: remove rew entries from a Cursor hooks event array, then append new entry.
fn upsert_cursor_event(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    event_key: &str,
    entry: serde_json::Value,
) {
    if let Some(arr) = hooks_obj.get_mut(event_key).and_then(|v| v.as_array_mut()) {
        arr.retain(|e| {
            !e.get("command")
                .and_then(|c| c.as_str())
                .map(|c| c.contains("rew hook"))
                .unwrap_or(false)
        });
        arr.push(entry);
    } else {
        hooks_obj.insert(event_key.to_string(), serde_json::json!([entry]));
    }
}

fn install_cursor_hooks(rew_bin: &str) -> Result<(), String> {
    let hooks_path = cursor_hooks_path()
        .ok_or("未检测到 Cursor 安装")?;

    let mut root = read_cursor_hooks(&hooks_path)?;

    let hooks_obj = root
        .get_mut("hooks")
        .and_then(|v| v.as_object_mut())
        .ok_or("hooks.json 结构异常")?;

    // preToolUse — scope check before Write/Shell tool calls
    upsert_cursor_event(hooks_obj, "preToolUse", serde_json::json!({
        "command": format!("{} hook pre-tool --source cursor", rew_bin),
        "matcher": "Write|Shell"
    }));

    // postToolUse — record changes after Write/Shell tool calls
    upsert_cursor_event(hooks_obj, "postToolUse", serde_json::json!({
        "command": format!("{} hook post-tool --source cursor", rew_bin),
        "matcher": "Write|Shell"
    }));

    // afterFileEdit — record file edits (covers Tab edits too)
    upsert_cursor_event(hooks_obj, "afterFileEdit", serde_json::json!({
        "command": format!("{} hook post-tool --source cursor", rew_bin)
    }));

    // beforeSubmitPrompt — create task on prompt submit
    upsert_cursor_event(hooks_obj, "beforeSubmitPrompt", serde_json::json!({
        "command": format!("{} hook prompt --source cursor", rew_bin)
    }));

    // stop — close task
    upsert_cursor_event(hooks_obj, "stop", serde_json::json!({
        "command": format!("{} hook stop --source cursor", rew_bin)
    }));

    let content = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("JSON 序列化失败: {}", e))?;
    std::fs::write(&hooks_path, content)
        .map_err(|e| format!("写入失败: {}", e))?;

    Ok(())
}

fn remove_cursor_hooks(_rew_bin: &str) -> Result<(), String> {
    let hooks_path = cursor_hooks_path()
        .ok_or("未检测到 Cursor 安装")?;

    if !hooks_path.exists() {
        return Ok(());
    }

    let mut root = read_cursor_hooks(&hooks_path)?;

    if let Some(hooks_obj) = root.get_mut("hooks").and_then(|v| v.as_object_mut()) {
        for (_key, entries) in hooks_obj.iter_mut() {
            if let Some(arr) = entries.as_array_mut() {
                arr.retain(|entry| {
                    !entry
                        .get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.contains("rew hook"))
                        .unwrap_or(false)
                });
            }
        }
        // Remove empty event arrays
        let empty_keys: Vec<String> = hooks_obj
            .iter()
            .filter(|(_, v)| v.as_array().map(|a| a.is_empty()).unwrap_or(false))
            .map(|(k, _)| k.clone())
            .collect();
        for key in empty_keys {
            hooks_obj.remove(&key);
        }
    }

    // If hooks object is empty, remove the file entirely
    let hooks_empty = root
        .get("hooks")
        .and_then(|v| v.as_object())
        .map(|o| o.is_empty())
        .unwrap_or(true);

    if hooks_empty {
        let _ = std::fs::remove_file(&hooks_path);
    } else {
        let new_content = serde_json::to_string_pretty(&root)
            .map_err(|e| format!("JSON 序列化失败: {}", e))?;
        std::fs::write(&hooks_path, new_content)
            .map_err(|e| format!("写入失败: {}", e))?;
    }

    Ok(())
}

// ================================================================
// CodeBuddy
// ================================================================

/// CodeBuddy uses the same settings.json hook format as Claude Code,
/// but lives in ~/.codebuddy/. The tool_name set includes IDE-style
/// aliases (write_to_file, replace_in_file, execute_command) because
/// CodeBuddy in IDE mode sends those instead of Write/Edit/Bash.

fn codebuddy_settings_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let dir = home.join(".codebuddy");
    if dir.exists() {
        Some(dir.join("settings.json"))
    } else {
        None
    }
}

fn detect_codebuddy(rew_bin: &str) -> Option<AiToolInfo> {
    let home = dirs::home_dir()?;
    let dir = home.join(".codebuddy");
    if !dir.exists() {
        return None;
    }
    let settings_path = dir.join("settings.json");
    let hook_installed = check_settings_json_hooks(&settings_path, rew_bin);
    Some(AiToolInfo {
        id: "codebuddy".to_string(),
        name: "CodeBuddy".to_string(),
        hook_installed,
        config_path: Some(settings_path.to_string_lossy().to_string()),
    })
}

/// Matcher covering both CLI-style and IDE-style tool names.
const CODEBUDDY_TOOL_MATCHER: &str =
    "Write|write_to_file|Edit|replace_in_file|MultiEdit|Bash|execute_command|Delete|delete_file|remove_file";

fn install_codebuddy_hooks(rew_bin: &str) -> Result<(), String> {
    let settings_path = codebuddy_settings_path()
        .ok_or("未检测到 CodeBuddy 安装")?;

    let mut settings = read_or_create_settings_json(&settings_path)?;

    let hooks_section = ensure_hooks_section(&mut settings)?;

    let source = "codebuddy";
    add_rew_hook(
        hooks_section, "PreToolUse", CODEBUDDY_TOOL_MATCHER,
        make_hook_command(rew_bin, "pre-tool", source),
    );
    add_rew_hook(
        hooks_section, "UserPromptSubmit", "",
        make_hook_command(rew_bin, "prompt", source),
    );
    add_rew_hook(
        hooks_section, "PostToolUse", CODEBUDDY_TOOL_MATCHER,
        make_hook_command(rew_bin, "post-tool", source),
    );
    add_rew_hook(
        hooks_section, "Stop", "",
        make_hook_command(rew_bin, "stop", source),
    );

    write_settings_json(&settings_path, &settings)
}

fn remove_codebuddy_hooks(_rew_bin: &str) -> Result<(), String> {
    let settings_path = codebuddy_settings_path()
        .ok_or("未检测到 CodeBuddy 安装")?;
    remove_rew_hooks_from_settings_json(&settings_path)
}

// ================================================================
// WorkBuddy
// ================================================================

/// WorkBuddy is functionally identical to CodeBuddy but lives in
/// ~/.workbuddy/. Kept as a separate block so future divergence
/// (different tool names, extra events, etc.) doesn't require
/// untangling shared code.

fn workbuddy_settings_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let dir = home.join(".workbuddy");
    if dir.exists() {
        Some(dir.join("settings.json"))
    } else {
        None
    }
}

fn detect_workbuddy(rew_bin: &str) -> Option<AiToolInfo> {
    let home = dirs::home_dir()?;
    let dir = home.join(".workbuddy");
    if !dir.exists() {
        return None;
    }
    let settings_path = dir.join("settings.json");
    let hook_installed = check_settings_json_hooks(&settings_path, rew_bin);
    Some(AiToolInfo {
        id: "workbuddy".to_string(),
        name: "WorkBuddy".to_string(),
        hook_installed,
        config_path: Some(settings_path.to_string_lossy().to_string()),
    })
}

/// WorkBuddy uses the same tool names as CodeBuddy.
const WORKBUDDY_TOOL_MATCHER: &str = CODEBUDDY_TOOL_MATCHER;

fn install_workbuddy_hooks(rew_bin: &str) -> Result<(), String> {
    let settings_path = workbuddy_settings_path()
        .ok_or("未检测到 WorkBuddy 安装")?;

    let mut settings = read_or_create_settings_json(&settings_path)?;

    let hooks_section = ensure_hooks_section(&mut settings)?;

    let source = "workbuddy";
    add_rew_hook(
        hooks_section, "PreToolUse", WORKBUDDY_TOOL_MATCHER,
        make_hook_command(rew_bin, "pre-tool", source),
    );
    add_rew_hook(
        hooks_section, "UserPromptSubmit", "",
        make_hook_command(rew_bin, "prompt", source),
    );
    add_rew_hook(
        hooks_section, "PostToolUse", WORKBUDDY_TOOL_MATCHER,
        make_hook_command(rew_bin, "post-tool", source),
    );
    add_rew_hook(
        hooks_section, "Stop", "",
        make_hook_command(rew_bin, "stop", source),
    );

    write_settings_json(&settings_path, &settings)
}

fn remove_workbuddy_hooks(_rew_bin: &str) -> Result<(), String> {
    let settings_path = workbuddy_settings_path()
        .ok_or("未检测到 WorkBuddy 安装")?;
    remove_rew_hooks_from_settings_json(&settings_path)
}

// ================================================================
// Shared helpers for settings.json-style tools
// ================================================================

fn make_hook_command(rew_bin: &str, subcommand: &str, source: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": format!("{} hook {} --source {}", rew_bin, subcommand, source)
    })
}

fn check_settings_json_hooks(settings_path: &PathBuf, _rew_bin: &str) -> bool {
    if !settings_path.exists() {
        return false;
    }
    let content = match std::fs::read_to_string(settings_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    content.contains("rew hook")
}

fn read_or_create_settings_json(settings_path: &PathBuf) -> Result<serde_json::Value, String> {
    if settings_path.exists() {
        let content = std::fs::read_to_string(settings_path)
            .map_err(|e| format!("读取失败: {}", e))?;
        Ok(serde_json::from_str(&content).unwrap_or(serde_json::json!({})))
    } else {
        Ok(serde_json::json!({}))
    }
}

fn ensure_hooks_section(settings: &mut serde_json::Value) -> Result<&mut serde_json::Map<String, serde_json::Value>, String> {
    let obj = settings
        .as_object_mut()
        .ok_or("settings.json 不是有效的 JSON 对象")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".to_string(), serde_json::json!({}));
    }

    obj.get_mut("hooks")
        .unwrap()
        .as_object_mut()
        .ok_or_else(|| "hooks 不是有效的 JSON 对象".to_string())
}

fn write_settings_json(settings_path: &PathBuf, settings: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {}", e))?;
    }

    let content = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("JSON 序列化失败: {}", e))?;
    std::fs::write(settings_path, content)
        .map_err(|e| format!("写入失败: {}", e))?;

    Ok(())
}

fn remove_rew_hooks_from_settings_json(settings_path: &PathBuf) -> Result<(), String> {
    if !settings_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(settings_path)
        .map_err(|e| format!("读取失败: {}", e))?;

    if !content.contains("rew hook") {
        return Ok(());
    }

    let mut settings: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("JSON 解析失败: {}", e))?;

    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
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
                        true
                    }
                });
            }
        }

        let empty_events: Vec<String> = hooks
            .iter()
            .filter(|(_, v)| v.as_array().map(|a| a.is_empty()).unwrap_or(false))
            .map(|(k, _)| k.clone())
            .collect();
        for key in empty_events {
            hooks.remove(&key);
        }
    }

    let new_content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("JSON 序列化失败: {}", e))?;
    std::fs::write(settings_path, new_content)
        .map_err(|e| format!("写入失败: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_tools_preserve_non_rew_hooks_on_remove() {
        let dir = tempfile::TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");

        let mut settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "Write",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "existing-tool hook post-tool"
                            }
                        ]
                    }
                ]
            }
        });

        let hooks_section = ensure_hooks_section(&mut settings).unwrap();
        add_rew_hook(
            hooks_section,
            "PostToolUse",
            CODEBUDDY_TOOL_MATCHER,
            make_hook_command("/tmp/rew", "post-tool", "codebuddy"),
        );
        write_settings_json(&settings_path, &settings).unwrap();

        remove_rew_hooks_from_settings_json(&settings_path).unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        let entries = saved["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let command = entries[0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(command, "existing-tool hook post-tool");
    }

    #[test]
    fn codebuddy_and_workbuddy_matchers_cover_ide_style_tools() {
        for matcher in [CODEBUDDY_TOOL_MATCHER, WORKBUDDY_TOOL_MATCHER] {
            assert!(matcher.contains("write_to_file"));
            assert!(matcher.contains("replace_in_file"));
            assert!(matcher.contains("execute_command"));
            assert!(matcher.contains("delete_file"));
        }
    }
}
