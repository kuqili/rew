//! `rew install` / `rew uninstall` — AI tool hook management.
//!
//! Delegates to `rew_core::hooks` for the actual injection/removal logic.
//! Uses `rew_core::rew_cli_bin_path()` as the stable binary path.

use crate::display;
use rew_core::error::RewResult;

/// Detect AI tools and inject hooks.
pub fn install() -> RewResult<()> {
    let rew_bin = rew_core::rew_cli_bin_path()
        .to_string_lossy()
        .to_string();

    println!(
        "{} 正在检测 AI 工具并注入 hook...",
        display::info_prefix()
    );

    let tools = rew_core::hooks::detect_ai_tools(&rew_bin);
    let mut installed_count = 0;

    for tool in &tools {
        if tool.hook_installed {
            println!(
                "{} {} hook 已存在，跳过",
                display::info_prefix(),
                tool.name
            );
            installed_count += 1;
            continue;
        }

        match rew_core::hooks::install_hook(&tool.id, &rew_bin) {
            Ok(()) => {
                println!(
                    "{} {} hook 已注入",
                    display::success_prefix(),
                    tool.name
                );
                installed_count += 1;
            }
            Err(e) => {
                println!("  ⚠ {} hook 注入失败: {}", tool.name, e);
            }
        }
    }

    if installed_count == 0 && tools.is_empty() {
        println!("  未检测到已安装的 AI 工具，将使用纯文件监听模式");
    }

    // Generate default .rewscope if not exists
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
    let rew_bin = rew_core::rew_cli_bin_path()
        .to_string_lossy()
        .to_string();

    let tools = rew_core::hooks::detect_ai_tools(&rew_bin);
    for tool in &tools {
        if tool.hook_installed {
            match rew_core::hooks::uninstall_hook(&tool.id, &rew_bin) {
                Ok(()) => {
                    println!(
                        "{} {} hook 已移除",
                        display::success_prefix(),
                        tool.name
                    );
                }
                Err(e) => {
                    println!("  ⚠ {} hook 移除失败: {}", tool.name, e);
                }
            }
        }
    }

    println!(
        "{} 所有 rew hook 已清理",
        display::success_prefix()
    );

    Ok(())
}
