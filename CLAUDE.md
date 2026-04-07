
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

**Current feature:** #3: APFS 快照引擎与存储生命周期管理：封装 tmutil CLI 实现快照的创建/列举/挂载/恢复/删除全流程。实现分级保留策略（1小时内全保留、24小时内每小时1个、30天内每天1个），异常快照双倍保留，用户标记快照永久保留。快照元数据写入 SQLite，实际文件数据完全依赖 APFS CoW。
**Steps:**
- 实现 TmutilWrapper：封装 tmutil localsnapshot（创建）、listlocalsnapshots（列举）、deletelocalsnapshots（删除）命令，解析输出格式
- 实现 SnapshotEngine for macOS：接收 EventBatch，调用 TmutilWrapper 创建快照，将元数据（时间戳、触发类型、文件变更统计）写入 SQLite
- 实现 RestoreEngine：通过 tmutil restore 执行恢复，支持 dry_run 模式（预览恢复影响但不执行），恢复前自动创建当前状态快照作为保险
- 实现 StorageManager：分级保留策略清理器，每次新快照创建后触发，按规则清理过期快照；磁盘占用超过用户配置阈值（默认 10GB）时发出告警
- 实现快照标记功能：用户可将任意快照标记为 pinned，pinned 快照不参与自动清理
- 处理边界情况：tmutil 权限不足时的友好提示、快照被 OS 自动删除时更新 DB、并发快照请求的串行化

**Rules:**
- Do NOT remove or weaken existing tests
- Self-verify all features before considering them done
- Commit with descriptive messages
- Append progress summary to claude-progress.txt before exiting

