
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

**Current feature:** #6: Tauri GUI：系统托盘 + 时间线 Web UI + 首次启动向导。系统托盘常驻显示运行状态，点击展开状态菜单。主窗口为时间线界面，可视化展示所有快照、异常事件标记、文件变更摘要，支持一键恢复。首次启动引导用户选择保护目录。这是产品化的关键差异点——让非技术用户也能轻松使用。
**Steps:**
- 实现系统托盘：使用 Tauri SystemTray API，图标显示运行状态（绿色=正常/黄色=有告警/灰色=已暂停），右键菜单：查看时间线/暂停保护/恢复保护/退出
- 实现首次启动向导页面：欢迎语 → 自动选中 Desktop/Documents/Downloads → 允许添加自定义目录 → 开启保护 → 窗口最小化到托盘
- 实现时间线主界面：左侧按日期分组的快照列表（时间戳 + 触发类型标签 + 变更统计），右侧选中快照的详情（文件变更列表：新增/修改/删除分类展示）
- 实现恢复交互流：选中快照 → 点击恢复 → 弹出预览面板（显示将恢复的文件列表 + 将被覆盖的文件列表）→ 确认恢复 → 进度条 → 完成提示
- 实现 Tauri Commands：将 rew-core 的 list_snapshots / restore / get_status / update_config 暴露为 #[tauri::command]，前端通过 invoke 调用
- 实现异常通知联动：收到异常告警时，托盘图标变黄，系统通知点击「查看详情」直接打开时间线并定位到异常快照
- 前端技术选型：轻量方案（Vanilla TS + Tailwind CSS 或 Solid.js），保持安装包小（< 10MB 目标）

**Rules:**
- Do NOT remove or weaken existing tests
- Self-verify all features before considering them done
- Commit with descriptive messages
- Append progress summary to claude-progress.txt before exiting

