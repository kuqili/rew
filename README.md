# rew — AI 时代的文件安全网

rew 实时监控你的文件，在 AI 工具（Claude Code、Cursor 等）操作文件时自动备份。如果 AI 误删或改错了文件，一键撤销恢复。

## 项目结构

```
rew/
├── Cargo.toml              # Rust workspace（rew-core / rew-cli / src-tauri）
│
├── crates/
│   ├── rew-core/           # 核心库（无 Tauri 依赖，可独立测试）
│   │   └── src/
│   │       ├── types.rs    #   核心类型（Task, Change, Snapshot…）
│   │       ├── db.rs       #   SQLite（tasks / changes / snapshots）
│   │       ├── objects.rs  #   内容寻址对象存储（SHA-256 + clonefile）
│   │       ├── scanner.rs  #   启动时全量扫描器（增量 manifest）
│   │       ├── pipeline.rs #   FSEvents → Shadow → EventProcessor 流水线
│   │       ├── restore.rs  #   撤销引擎（TaskRestoreEngine）
│   │       ├── scope.rs    #   .rewscope 作用域规则引擎
│   │       ├── detector/   #   异常检测规则（8 条）
│   │       ├── backup/     #   clonefile FFI + BackupEngine
│   │       └── config.rs   #   配置管理（~/.rew/config.toml）
│   │
│   └── rew-cli/            # CLI 工具（rew install / hook / undo / list…）
│       └── src/commands/   #   hook, install, status, list, show, diff, undo
│
├── src-tauri/              # Tauri 2 桌面端后端（Rust）
│   └── src/
│       ├── commands.rs     #   Tauri IPC 命令（task / scan / analyze…）
│       ├── daemon.rs       #   后台守护（FSEvents + 扫描 + 异常检测）
│       ├── state.rs        #   共享状态（ScanProgress, AppState）
│       └── tray.rs         #   系统托盘
│
├── gui/                    # 前端 GUI（React + Vite + Tailwind）
│   ├── src/
│   │   ├── components/     #   UI 组件（Sidebar, TaskTimeline, DiffViewer…）
│   │   ├── hooks/          #   React hooks（useTasks, useScanProgress）
│   │   └── lib/            #   工具函数 + Tauri IPC 封装
│   ├── index.html          #   WebView 入口
│   ├── vite.config.ts      #   Vite 构建配置
│   ├── tailwind.config.js  #   Tailwind 主题（Sourcetree 配色）
│   └── package.json        #   前端依赖（React / Tailwind / Tauri API）
│
├── launchagent/            # macOS LaunchAgent plist 模板
├── tests/                  # 跨 crate 集成测试
├── docs/                   # 产品文档（调研 / 需求 / 技术方案）
│
└── package.json            # 根级脚本（pnpm dev / pnpm build → tauri）
```

## 快速开始

```bash
# 1. 安装前端依赖
cd gui && pnpm install && cd ..

# 2. 安装根级 Tauri CLI
pnpm install

# 3. 开发模式（热重载）
pnpm dev          # 等同于 tauri dev，自动启动 gui/ 的 Vite 服务

# 4. 构建桌面应用
pnpm build

# 5. 构建 CLI 工具
cargo build -p rew-cli --release
```

## 核心功能

### 自动备份
- 启动时全量扫描所有保护目录（APFS clonefile，秒级完成）
- FSEvents 实时监听文件变更
- Shadow 机制：文件被删前立即备份内容

### 一键撤销
- 点击任务 → 查看改了什么 → 点撤销 → 文件恢复
- 支持 Created(删除) / Modified(回退) / Deleted(恢复) / Renamed

### AI 工具集成
```bash
rew install    # 注入 Hook 到 Claude Code / Cursor
```

### 默认不备份
- `.app` 包、`.dmg/.pkg/.iso` 安装包
- `node_modules/.git/target` 开发产物
- `.DS_Store` 等系统临时文件

## 运行时数据

存储在 `~/.rew/`：
- `config.toml` — 配置（保护目录、忽略规则）
- `snapshots.db` — SQLite 数据库
- `objects/` — 文件备份（clonefile CoW，与原文件共享空间）
- `.scan_manifest.json` — 扫描增量记录
- `.scan_status.json` — per-directory 扫描状态

## 系统要求

- macOS 11.0+
- APFS 文件系统
