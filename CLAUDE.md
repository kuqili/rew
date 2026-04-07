
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

**Current feature:** #4: 异常检测规则引擎与系统通知：实现 7 条异常检测规则的滑动窗口规则引擎，按 CRITICAL/HIGH/MEDIUM 分级输出告警。CRITICAL 和 HIGH 立刻触发快照+系统通知，MEDIUM 延迟聚合后通知。macOS 系统通知支持操作按钮（查看详情/忽略）。
**Steps:**
- 实现 AnomalyDetector：维护 30 秒滑动窗口内的事件统计，逐条评估 RULE-01 到 RULE-07
- 实现规则优先级合并：同一窗口内多条规则命中时取最高级别；CRITICAL 立即中断窗口并触发快照
- 实现告警去重：同一目录同一规则在 5 分钟内不重复告警
- 集成 macOS 系统通知（notify-rust crate 或 NSUserNotification API）：通知标题、影响文件数、目录路径、操作按钮
- 实现 MEDIUM 级别聚合通知：收集 2 分钟内所有 MEDIUM 告警，合并为一条汇总通知发送
- 将 AnomalyDetector 接入事件管道：EventProcessor → AnomalyDetector → (触发 SnapshotEngine + Notifier)

**Rules:**
- Do NOT remove or weaken existing tests
- Self-verify all features before considering them done
- Commit with descriptive messages
- Append progress summary to claude-progress.txt before exiting

