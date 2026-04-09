# rew — AI 时代的文件安全网

rew 实时监控你的文件，在 AI 工具（Claude Code、Cursor 等）操作文件时自动备份。如果 AI 误删或改错了文件，一键撤销恢复。

## 项目结构

```
rew/
├── src/                    # 前端 React + Tailwind (Sourcetree 风格 GUI)
│   ├── components/         #   UI 组件 (Sidebar, Timeline, Settings...)
│   ├── hooks/              #   React hooks (useTasks, useScanProgress)
│   ├── lib/                #   工具函数 + Tauri IPC 封装
│   ├── App.tsx             #   入口组件
│   └── index.css           #   Tailwind + 主题样式
├── src-tauri/              # Tauri 2 桌面应用后端
│   ├── src/
│   │   ├── commands.rs     #   IPC 命令 (15个: task/scan/analyze...)
│   │   ├── daemon.rs       #   后台守护 (FSEvents + 扫描 + 异常检测)
│   │   ├── state.rs        #   共享状态 (ScanProgress, AppState)
│   │   ├── lib.rs          #   Tauri 启动 + 插件注册
│   │   └── tray.rs         #   系统托盘
│   └── capabilities/       #   Tauri 2 权限配置
├── crates/
│   ├── rew-core/           # 核心库 (无 Tauri 依赖)
│   │   ├── src/
│   │   │   ├── objects.rs  #   Content-addressable 对象存储 (SHA-256 + fast key)
│   │   │   ├── scanner.rs  #   全量扫描器 (store_fast, 增量 manifest)
│   │   │   ├── restore.rs  #   撤销引擎 (TaskRestoreEngine)
│   │   │   ├── pipeline.rs #   FSEvents → 批处理 Pipeline + Shadow 机制
│   │   │   ├── scope.rs    #   .rewscope 规则引擎
│   │   │   ├── backup/     #   clonefile FFI + BackupEngine
│   │   │   ├── db.rs       #   SQLite (tasks, changes, snapshots)
│   │   │   ├── config.rs   #   配置管理 (~/.rew/config.toml)
│   │   │   ├── detector/   #   异常检测规则 (8 条)
│   │   │   └── types.rs    #   核心类型 (Task, Change, Snapshot...)
│   │   └── tests/          #   集成测试
│   └── rew-cli/            # CLI 工具
│       └── src/commands/   #   hook, install, undo, list, show, diff
├── docs/                   # 产品文档 (调研/需求/技术方案)
├── resources/              # LaunchAgent plist
├── tests/                  # 跨 crate 集成测试
├── Cargo.toml              # Rust workspace 配置
├── package.json            # Node 依赖 (React, Tailwind, Tauri API)
├── tailwind.config.js      # Tailwind 主题 (Sourcetree 配色)
├── vite.config.ts          # Vite 构建配置
└── index.html              # Tauri WebView 入口
```

## 快速开始

```bash
# 安装依赖
pnpm install

# 开发模式 (热重载)
pnpm tauri dev

# 构建桌面应用
pnpm tauri build

# 构建 CLI
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
