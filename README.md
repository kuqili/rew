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

## 测试

文件变更语义相关的回归，统一从这个脚本入口跑：

```bash
./scripts/test-change-semantics.sh
```

这个脚本会串起当前最关键的 4 组测试：

- `change_tracking.rs`：核心 DB / baseline / reconcile / dedup 回归
- `git_semantics.rs`：真实 Git 对拍
- `processor::merge_logic_tests`：3 秒窗口归并与 dynamic pause
- `watcher::filter` + `daemon::tests`：过滤规则与 FSEvent 路由

更详细的说明见：`docs/TESTING.md`

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
- `*.photoslibrary/private/**`、`*.photoslibrary/database/search/**` 等系统生成索引/分析数据

## 过滤规则分层

为了避免“有些路径实现了过滤，有些路径漏过滤”，当前项目的过滤规则按 **3 层** 设计。排查某个路径为什么被过滤/没被过滤时，优先按这个顺序看。

### 第 1 层：全局静态过滤（单一事实来源）

单一事实来源：

- `crates/rew-core/src/watcher/filter.rs`
- `PathFilter::builtin_patterns()`
- `PathFilter::should_ignore()`

这一层负责所有“默认就不应该记录”的路径，特点是：

- 对 scanner / watcher / daemon / hook 统一生效
- `PathFilter::new()` 会把用户自定义 `ignore_patterns` 合并到内置默认规则上
- 这是最核心、最稳定的一层

这一层包含 4 类规则：

1. **内置 glob 模式**
   例如：
   - `**/.git/**`
   - `**/node_modules/**`
   - `**/target/**`
   - `**/*.app/**`
   - `**/*.photoslibrary/private/**`
   - `**/*.photoslibrary/database/search/**`

2. **文件名快速过滤**
   例如：
   - `.DS_Store`
   - `*.tmp`
   - `*.tmp-*`
   - `.lock / .LOCK`
   - `-wal / -shm / -journal`
   - `.zcompdump*`

3. **HOME 顶层隐藏目录规则**
   - `~/.cargo/`、`~/.npm/`、`~/.cursor/` 这类工具运行数据默认忽略
   - `~/.ssh/`、`~/.gnupg/`、`~/.aws/`、`~/.kube/` 这类重要配置目录保留
   - `~/.zshrc`、`~/.gitconfig` 这类 HOME 根目录隐藏文件保留

4. **目录组件级兜底过滤**
   当 glob 对绝对路径匹配不稳定时，还会对祖先目录名做兜底判断：
   - `node_modules`
   - `.git`
   - `target`
   - `__pycache__`
   - `.rew`
   - `Library`
   - `.Trash`

### 第 2 层：按受保护目录的局部过滤

实现位置：

- `PathFilter::should_ignore_by_dir_config()`

这一层用于“只在某个 watch_dir 下生效”的规则，来源是每个受保护目录自己的配置：

- `exclude_dirs`
- `exclude_extensions`

特点：

- 不是全局忽略
- 只对命中的 watch_dir 生效
- scanner / daemon / hook / 部分命令查询都会用到

适合处理：

- 某个项目自己的 `dist/`
- 某个目录下的 `coverage/`
- 某类扩展名，例如 `.log`、`.sqlite`

### 第 3 层：运行时临时抑制

实现位置：

- `crates/rew-core/src/processor.rs`

这不是“永久过滤规则”，而是为了降低瞬时噪音的 **动态 pause**：

1. `package.json` / `Cargo.toml` / lockfile 变化后
   - 暂时抑制 `node_modules/`、`target/`

2. `.git/HEAD` 变化后
   - 暂时全局抑制一小段时间

这一层的特点：

- 只影响短时间内的事件流
- 不改变静态 ignore 配置
- 主要目的是避免包管理器 / Git 批量操作制造大量无价值噪音

## 过滤规则在哪里生效

当前这些模块都会用到过滤逻辑：

- `scanner`：启动时全量扫描
- `watcher`：FSEvents 原始事件入口
- `daemon`：批处理与入库前过滤
- `hook`：AI 工具路径过滤
- 部分命令查询路径：避免把已知噪音带回前端

所以如果一个路径被纳入“应该默认过滤”，正确做法是：

- **优先加到 `PathFilter`**
- 不要只在前端隐藏
- 也不要只在某一个模块里打补丁

## 当前关于 Photos Library 的处理

针对 `Photos Library.photoslibrary`，当前默认只过滤已知高噪音、低用户价值的系统目录：

- `**/*.photoslibrary/private/**`
- `**/*.photoslibrary/database/search/**`

原因：

- 这些目录主要是 Photos / Spotlight 的分析、索引、模型和状态文件
- 变更频繁，但通常不是用户主动编辑内容

同时不会一刀切忽略整个 `*.photoslibrary`，因为其中仍可能包含真正重要的用户资产。

## 运行时数据

存储在 `~/.rew/`：
- `config.toml` — 配置（保护目录、忽略规则）
- `snapshots.db` — 唯一主 SQLite 数据库（任务、变更、扫描基线等）
- `objects/` — 对象存储（SHA-256 / fast key 对应的文件备份）
- `backups/` — 按 snapshot ID 划分的备份目录，随 snapshot cleanup 一起删除
- `logs/` — 运行日志，按天轮转，默认保留 7 天
- `.scan_status.json` — 前端扫描进度状态
- `.shadow_hashes/` — shadow 层的短期路径到对象哈希映射，会自动清理陈旧项
- `.setup_done` — 初始化完成标记

说明：
- `.scan_manifest.json` 已废弃，不再作为运行时依赖；旧文件启动时会自动清理。
- `hook_debug.log` 默认不写；仅在显式设置环境变量 `REW_HOOK_DEBUG=1` 时生成，并在过大时自动截断。

## 系统要求

- macOS 11.0+
- APFS 文件系统
