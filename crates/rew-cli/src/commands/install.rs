//! `rew install` / `rew uninstall` — LaunchAgent management.

use crate::display;
use rew_core::error::RewResult;
use rew_core::launchd;

/// Install the LaunchAgent for auto-start on login.
pub fn install() -> RewResult<()> {
    // Find the rew binary path
    let exe = std::env::current_exe().map_err(|e| {
        rew_core::error::RewError::Config(format!("Could not determine rew binary path: {}", e))
    })?;

    println!(
        "{} 正在安装 LaunchAgent...",
        display::info_prefix()
    );

    launchd::install(&exe)?;

    println!(
        "{} LaunchAgent 已安装",
        display::success_prefix()
    );
    println!(
        "  plist: {}",
        launchd::plist_path().display()
    );
    println!("  rew 将在下次登录时自动启动");
    println!(
        "  使用 {} 立即启动守护进程",
        colored::Colorize::cyan("rew daemon")
    );

    Ok(())
}

/// Uninstall the LaunchAgent.
pub fn uninstall() -> RewResult<()> {
    if !launchd::is_installed() {
        println!(
            "{} LaunchAgent 未安装，无需卸载",
            display::info_prefix()
        );
        return Ok(());
    }

    println!(
        "{} 正在卸载 LaunchAgent...",
        display::info_prefix()
    );

    launchd::uninstall()?;

    println!(
        "{} LaunchAgent 已卸载",
        display::success_prefix()
    );
    println!("  rew 将不再在登录时自动启动");

    Ok(())
}
