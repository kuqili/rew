
---
# Claude-Forever Harness Context

This project is managed by claude-forever, an autonomous coding harness.
IMPORTANT files to read at the start of each session:
- `claude-progress.txt` — session-by-session progress log (READ first, APPEND when done)
- `git log --oneline -10` — recent commit history

**Goal:** 结合/Users/kuqili/Downloads/tansuo/rew/docs下的方案文档，结合实际需求痛点，完成rew项目的所有能力，并且做到可产品化


**Current feature:** #1: Tauri v2 项目初始化与核心架构搭建：创建完整的 Tauri v2 + Rust 项目骨架，定义所有核心模块的 trait 接口和数据结构，建立跨平台抽象层。包括 Cargo workspace 配置、模块划分（watcher/engine/detector/notifier/storage/cli）、公共数据类型定义、错误处理框架。
**Steps:**
- 创建 Tauri v2 项目：pnpm create tauri-app，选择 Rust + React/Vanilla 前端
- 设计 Cargo workspace 结构：rew-core（核心逻辑库）、rew-tauri（Tauri 应用入口）、rew-cli（CLI 入口），实现关注点分离
- 定义核心 trait 接口：FileWatcher trait、SnapshotEngine trait、StorageBackend trait，为未来 Windows 适配预留扩展点
- 定义公共数据结构：FileEvent { path, kind, timestamp }、EventBatch { events, window_start, window_end }、Snapshot { id, timestamp, trigger_type, metadata }、AnomalySignal { kind, severity, affected_files }
- 建立统一错误处理：自定义 RewError 枚举，覆盖 IO/DB/Snapshot/Config 四类错误
- 创建 config.toml 配置模块：watch_dirs（默认 ~/Desktop, ~/Documents, ~/Downloads）、ignore_patterns、anomaly_rules、retention_policy
- 集成 SQLite（rusqlite）：创建 ~/.rew/snapshots.db，定义 snapshots 表 schema（id, timestamp, trigger_type, os_snapshot_ref, files_added, files_modified, files_deleted, metadata_json）

**Rules:**
- Do NOT remove or weaken existing tests
- Self-verify all features before considering them done
- Commit with descriptive messages
- Append progress summary to claude-progress.txt before exiting

