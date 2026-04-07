
---
# Claude-Forever Harness Context

This project is managed by claude-forever, an autonomous coding harness.
IMPORTANT files to read at the start of each session:
- `claude-progress.txt` — session-by-session progress log (READ first, APPEND when done)
- `git log --oneline -10` — recent commit history

**Goal:** 结合/Users/kuqili/Downloads/tansuo/rew/docs下的方案文档，结合实际需求痛点，完成rew项目的所有能力，并且做到可产品化

**Completed features:**
- [x] Feature #1: Tauri v2 项目初始化与核心架构搭建：创建完整的 Tauri v2 + Rust 项目骨架，定义所有核心模块的 trait 接口和数据结构，建立跨平台抽象层。包括 Cargo workspace 配置、模块划分（watcher/engine/detector/notifier/storage/cli）、公共数据类型定义、错误处理框架。
- [x] Feature #2: 文件监听与事件处理引擎：基于 notify crate 实现 macOS FSEvents 文件监听，配合 tokio 异步运行时实现 30 秒滑动窗口事件聚合、去重、节流。包括智能噪音过滤（忽略 node_modules/.git/.DS_Store 等）、package.json 变更触发的动态过滤暂停机制。
- [x] Feature #3: APFS 快照引擎与存储生命周期管理：封装 tmutil CLI 实现快照的创建/列举/挂载/恢复/删除全流程。实现分级保留策略（1小时内全保留、24小时内每小时1个、30天内每天1个），异常快照双倍保留，用户标记快照永久保留。快照元数据写入 SQLite，实际文件数据完全依赖 APFS CoW。
- [x] Feature #4: 异常检测规则引擎与系统通知：实现 7 条异常检测规则的滑动窗口规则引擎，按 CRITICAL/HIGH/MEDIUM 分级输出告警。CRITICAL 和 HIGH 立刻触发快照+系统通知，MEDIUM 延迟聚合后通知。macOS 系统通知支持操作按钮（查看详情/忽略）。
- [x] Feature #5: CLI 工具：实现 rew 命令行界面，包括 rew status（查看运行状态和快照列表）、rew restore（交互式选择恢复点并执行恢复）、rew config（管理保护目录和配置）、rew list（查看快照详情）、rew pin（标记重要快照）。CLI 是非技术用户的主要恢复入口，交互必须极简。
- [x] Feature #6: Tauri GUI：系统托盘 + 时间线 Web UI + 首次启动向导。系统托盘常驻显示运行状态，点击展开状态菜单。主窗口为时间线界面，可视化展示所有快照、异常事件标记、文件变更摘要，支持一键恢复。首次启动引导用户选择保护目录。这是产品化的关键差异点——让非技术用户也能轻松使用。

**Current feature:** #7: 开机自启、生产打包与集成测试：配置 macOS LaunchAgent 实现开机自启，构建 .dmg 安装包（< 10MB），编写端到端集成测试覆盖完整用户旅程（安装 → 监听 → 异常检测 → 恢复）。确保产品级健壮性：崩溃自动重启、优雅退出、日志轮转。
**Steps:**
- 实现 LaunchAgent 注册/注销：生成 ~/Library/LaunchAgents/com.rew.agent.plist，支持 rew install（注册自启）和 rew uninstall（移除自启）
- 实现守护进程生命周期管理：优雅退出（SIGTERM 处理，完成当前快照后退出）、崩溃自动重启（LaunchAgent KeepAlive）、启动时 DB 完整性检查
- 实现日志模块：使用 tracing crate，日志写入 ~/.rew/rew.log，按天轮转，保留 7 天
- 配置 Tauri 打包：macOS .dmg 构建，应用签名配置（开发阶段可跳过），图标资源
- 编写集成测试：模拟完整用户旅程——创建测试目录 → 启动 rew 监听 → 批量删除文件 → 验证异常告警触发 → 验证快照创建 → 执行恢复 → 验证文件恢复正确
- 编写性能基准测试：持续运行 1 小时 CPU/内存采样、快照创建延迟 P99、大目录（10000+ 文件）下的事件处理吞吐量
- 创建 README 安装说明和用户指南（仅基础内容，不做过度文档化）

**Rules:**
- Do NOT remove or weaken existing tests
- Self-verify all features before considering them done
- Commit with descriptive messages
- Append progress summary to claude-progress.txt before exiting

