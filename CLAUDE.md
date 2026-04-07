
---
# Claude-Forever Harness Context

This project is managed by claude-forever, an autonomous coding harness.
IMPORTANT files to read at the start of each session:
- `claude-progress.txt` — session-by-session progress log (READ first, APPEND when done)
- `git log --oneline -10` — recent commit history

**Goal:** 结合/Users/kuqili/Downloads/tansuo/rew/docs下的方案文档，结合实际需求痛点，完成rew项目的所有能力，并且做到可产品化

**Completed features:**
- [x] Feature #1: Tauri v2 项目初始化与核心架构搭建：创建完整的 Tauri v2 + Rust 项目骨架，定义所有核心模块的 trait 接口和数据结构，建立跨平台抽象层。包括 Cargo workspace 配置、模块划分（watcher/engine/detector/notifier/storage/cli）、公共数据类型定义、错误处理框架。

**Current feature:** #2: 文件监听与事件处理引擎：基于 notify crate 实现 macOS FSEvents 文件监听，配合 tokio 异步运行时实现 30 秒滑动窗口事件聚合、去重、节流。包括智能噪音过滤（忽略 node_modules/.git/.DS_Store 等）、package.json 变更触发的动态过滤暂停机制。
**Steps:**
- 集成 notify crate（v6+），实现 MacOSWatcher：监听配置的目标目录，递归监听子目录
- 实现路径过滤器 PathFilter：基于 glob 模式匹配忽略列表（node_modules、.git、target、__pycache__、.DS_Store、Thumbs.db 等），支持用户在 config.toml 中自定义
- 实现 EventProcessor：使用 tokio::time::interval 和 HashMap<PathBuf, FileEvent> 实现 30 秒滑动窗口，同路径同类型事件去重，窗口结束时输出 EventBatch
- 实现动态过滤暂停：检测到 package.json/Cargo.toml 变化时暂停 node_modules/target 目录检测 60 秒；检测到 .git/HEAD 变化时暂停全局检测 10 秒
- 实现事件统计模块：每个 EventBatch 计算 files_added/files_modified/files_deleted 数量和总大小
- 将 FileWatcher 和 EventProcessor 通过 tokio mpsc channel 连接，形成异步事件管道

**Rules:**
- Do NOT remove or weaken existing tests
- Self-verify all features before considering them done
- Commit with descriptive messages
- Append progress summary to claude-progress.txt before exiting

