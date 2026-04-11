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
│   │       ├── types.rs        # 核心类型（Task, Change, Snapshot…）
│   │       ├── db.rs           # SQLite（tasks / changes / snapshots）
│   │       ├── objects.rs      # 内容寻址对象存储（SHA-256 + clonefile）
│   │       ├── scanner.rs      # 启动时全量扫描器（增量 manifest）
│   │       ├── pipeline.rs     # FSEvents → Shadow → EventProcessor 流水线
│   │       ├── processor.rs    # 事件处理器
│   │       ├── restore.rs      # 撤销引擎（TaskRestoreEngine）
│   │       ├── scope.rs        # .rewscope 作用域规则引擎
│   │       ├── storage.rs      # 存储层抽象
│   │       ├── diff.rs         # 文件差异计算
│   │       ├── hooks.rs        # Hook 注册与触发
│   │       ├── lifecycle.rs    # 任务生命周期管理
│   │       ├── logging.rs      # 结构化日志
│   │       ├── traits.rs       # 核心 trait 定义
│   │       ├── error.rs        # 错误类型
│   │       ├── config.rs       # 配置管理（~/.rew/config.toml）
│   │       ├── backup/         # clonefile FFI + BackupEngine + 复制策略
│   │       ├── detector/       # 异常检测规则（dedup / rules）
│   │       ├── notifier/       # 系统通知
│   │       ├── snapshot/       # 快照管理
│   │       └── watcher/        # FSEvents 文件监听
│   │
│   └── rew-cli/            # CLI 工具
│       └── src/commands/
│           ├── init.rs         # rew init（初始化保护目录）
│           ├── install.rs      # rew install（注入 AI 工具 Hook）
│           ├── hook.rs         # rew hook（Hook 回调入口）
│           ├── list.rs         # rew list（列出任务）
│           ├── show.rs         # rew show（查看任务详情）
│           ├── diff.rs         # rew diff（查看文件变更）
│           ├── restore.rs      # rew restore（恢复文件）
│           ├── undo.rs         # rew undo（撤销任务）
│           ├── pin.rs          # rew pin（固定任务）
│           ├── status.rs       # rew status（运行状态）
│           └── config.rs       # rew config（配置管理）
│
├── src-tauri/              # Tauri 2 桌面端后端（Rust）
│   └── src/
│       ├── commands.rs     # Tauri IPC 命令（task / scan / analyze…）
│       ├── daemon.rs       # 后台守护（FSEvents + 扫描 + 异常检测）
│       ├── state.rs        # 共享状态（ScanProgress, AppState）
│       └── tray.rs         # 系统托盘
│
├── gui/                    # 前端 GUI（React + Vite + Tailwind）
│   └── src/
│       ├── components/
│       │   ├── MainLayout.tsx      # 主布局
│       │   ├── Sidebar.tsx         # 侧边栏（任务列表）
│       │   ├── TaskTimeline.tsx    # 任务时间轴
│       │   ├── TaskDetail.tsx      # 任务详情面板
│       │   ├── DiffViewer.tsx      # 文件差异查看器
│       │   ├── RollbackPanel.tsx   # 回滚操作面板
│       │   ├── UndoConfirm.tsx     # 撤销确认对话框
│       │   ├── SettingsPanel.tsx   # 设置面板
│       │   ├── SetupWizard.tsx     # 初始化向导
│       │   └── Toolbar.tsx         # 顶部工具栏
│       ├── hooks/
│       │   ├── useTasks.ts         # 任务数据 hook
│       │   └── useScanProgress.ts  # 扫描进度 hook
│       └── lib/
│           ├── tauri.ts            # Tauri IPC 封装
│           ├── tools.ts            # AI 工具识别
│           └── format.ts           # 格式化工具函数
│
├── website/                # 产品官网（静态 HTML）
├── tests/                  # 跨 crate 集成测试
├── docs/                   # 产品文档（调研 / 需求 / 技术方案）
├── scripts/                # 部署脚本
│
└── package.json            # 根级脚本（pnpm dev / pnpm build → tauri）
```

## 快速开始

```bash
# 1. 安装所有依赖（根级 + gui/）
pnpm install
cd gui && pnpm install && cd ..

# 2. 开发模式（热重载）
pnpm dev          # tauri dev，自动启动 gui/ 的 Vite 服务

# 3. 构建桌面应用
pnpm build

# 4. 构建 CLI 工具
cargo build -p rew-cli --release
```

## 核心功能

### 自动备份
- 启动时全量扫描所有保护目录（APFS clonefile，秒级完成）
- FSEvents 实时监听文件变更
- Shadow 机制：文件被删前立即备份内容

### 一键撤销
- 点击任务 → 查看改了什么 → 点撤销 → 文件恢复
- 支持 Created（删除）/ Modified（回退）/ Deleted（恢复）/ Renamed（重命名还原）

### AI 工具集成
```bash
rew install    # 自动注入 Hook 到 Claude Code / Cursor
```

Hook 注入后，AI 工具的每次操作会被归组为一个"任务"，可在桌面 GUI 中按任务粒度查看和撤销。

### 默认忽略
- `.app` 包、`.dmg/.pkg/.iso` 安装包
- `node_modules/`、`.git/`、`target/` 等开发产物
- `.DS_Store` 等系统临时文件

## 运行时数据

存储在 `~/.rew/`：
- `config.toml` — 配置（保护目录、忽略规则）
- `snapshots.db` — SQLite 数据库
- `objects/` — 文件备份（clonefile CoW，与原文件共享磁盘空间）
- `.scan_manifest.json` — 扫描增量记录
- `.scan_status.json` — per-directory 扫描状态

## 系统要求

- macOS 11.0+
- APFS 文件系统
