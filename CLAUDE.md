
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

**Current feature:** #5: CLI 工具：实现 rew 命令行界面，包括 rew status（查看运行状态和快照列表）、rew restore（交互式选择恢复点并执行恢复）、rew config（管理保护目录和配置）、rew list（查看快照详情）、rew pin（标记重要快照）。CLI 是非技术用户的主要恢复入口，交互必须极简。
**Steps:**
- 集成 clap v4 构建命令行解析：rew status / restore / config / list / pin 子命令
- 实现 rew status：显示守护进程状态（运行中/已停止）、保护目录列表、最近 5 个快照摘要、存储用量
- 实现 rew list：按时间倒序列出快照，显示时间戳、触发类型图标（🔵自动/🔴异常/📌已标记）、文件变更统计
- 实现 rew restore：交互式（dialoguer crate）选择恢复点 → 显示恢复预览（影响文件列表）→ 用户确认 → 执行恢复 → 报告结果；也支持 --snapshot-id 直接指定
- 实现 rew config：add-dir/remove-dir 管理保护目录、show 显示当前配置、reset 恢复默认
- 实现 rew pin <snapshot-id>：标记/取消标记快照
- CLI 输出使用 colored/console crate 美化，关键信息用颜色区分

**Rules:**
- Do NOT remove or weaken existing tests
- Self-verify all features before considering them done
- Commit with descriptive messages
- Append progress summary to claude-progress.txt before exiting

