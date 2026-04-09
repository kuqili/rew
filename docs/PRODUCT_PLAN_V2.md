# rew — AI 时代的安全带

> Rewind your files. Undo AI mistakes.
> 看着。拦着。兜底。

---

## 一、产品定位

### 1.1 一句话定位

**rew = AI 时代的 Ctrl+Z**

AI 改了你的文件？`rew undo`，回到刚才。就像游戏里读档一样简单。

### 1.2 核心价值主张

三个动作，三层保护：

- **看着**（Watch）：AI 每次动你的文件，自动存档。你不需要记得 commit，不需要学任何命令
- **拦着**（Guard）：AI 要碰不该碰的地方，直接阻止。不是事后告警，是事前拦截
- **兜底**（Backup）：出了事，一键回去。不需要知道 git checkout HEAD~3 是什么意思

### 1.3 沃尔沃隐喻

1959 年沃尔沃发明三点式安全带，开放专利，让所有车都更安全。不是因为安全带让车跑得更快，是因为安全带让人敢开更快。

rew 想做的，就是 AI 时代的安全带。不绑定某个 AI 工具，保护的是文件本身。

---

## 二、目标用户

| 优先级 | 用户类型 | 特征 | 核心诉求 |
|--------|---------|------|---------|
| P0 | Vibe coder（非技术） | 用 Cursor/Replit/Lovable 做产品，不会 Git | 让我安全地用 AI，不用担心翻车 |
| P1 | 独立开发者 | 会 Git 但来不及 commit，AI 操作太快 | 在我来不及 commit 的间隙帮我兜底 |
| P2 | 通用 Agent 用户 | 用 AI agent 整理文件、批量操作 | 防止 AI 误删非项目文件 |
| P3 | 团队 Leader | 给团队加安全网 | 需要可视化的变更审计 |

---

## 三、技术架构

### 3.1 双引擎设计

```
┌─────────────────────────────────────────────────┐
│                 rew 保护引擎                      │
│                                                  │
│  引擎 A：Hook 驱动（AI 工具有 hook 支持时）        │
│  ├── UserPromptSubmit → 创建快照 + 记录用户意图    │
│  ├── PreToolUse → 作用域检查 + clonefile 备份      │
│  ├── PostToolUse → 记录变更结果（异步）            │
│  └── Stop → 打包成一条任务记录（异步）             │
│                                                  │
│  引擎 B：文件监听驱动（兜底）                       │
│  ├── chokidar 实时监听文件变化                     │
│  ├── 变更前 clonefile 自动备份                     │
│  ├── burst 检测 + debounce 聚合                    │
│  └── 启发式识别任务边界                            │
│                                                  │
│  两个引擎数据汇入统一时间线                         │
└─────────────────────────────────────────────────┘
```

引擎 A 优先。当 AI 工具支持 hook 时，rew 拿到精确的任务边界（prompt→stop）、用户意图（prompt 文本）、操作前拦截能力。

引擎 B 兜底。无 hook 支持时或引擎 A 覆盖不到的场景（如 AI 通过子进程间接操作文件），文件系统监听保底。

### 3.2 Hook 多平台适配

`rew install` 一条命令，自动检测已安装的 AI 工具并注入对应格式的 hook：

| AI 工具 | Hook 配置文件 | 支持的 Hook |
|---------|-------------|------------|
| Claude Code | `.claude/settings.json` | PreToolUse（可阻止）、PostToolUse、UserPromptSubmit、Stop |
| Cursor | `.cursor/hooks.json` | beforeShellExecution（可阻止）、afterFileEdit、beforeSubmitPrompt、stop |
| GitHub Copilot | `.github/hooks/rew.json` | preToolUse（可阻止）、postToolUse、userPromptSubmitted |
| CodeBuddy | 类 Claude Code 格式 | PreToolUse、PostToolUse、Stop |
| 无 Hook 工具 | 不注入 | 纯文件监听模式 |

所有 hook 配置调用同一个 `rew hook <event>` 命令，核心逻辑只写一份。

### 3.3 存储架构

#### 3.3.1 三级存储策略

```
┌──────────────────────────────────────────┐
│           Level 0：APFS 卷快照            │
│  tmutil localsnapshot（启动时一次）         │
│  零空间、覆盖全盘、最后兜底                  │
│  适用：macOS（APFS 文件系统）              │
└──────────────────────────────────────────┘
                    │
┌──────────────────────────────────────────┐
│        Level 1：文件级 CoW 克隆            │
│  APFS clonefile / btrfs reflink          │
│  O(1)、零空间、每次工具调用前执行            │
│  适用：macOS（APFS）、Linux（btrfs）       │
└──────────────────────────────────────────┘
                    │
┌──────────────────────────────────────────┐
│        Level 2：应用层 copy + 压缩         │
│  fs.copyFile + zstd 压缩                 │
│  兜底方案，适用于所有文件系统               │
│  适用：ext4、NTFS 等不支持 CoW 的系统      │
└──────────────────────────────────────────┘
```

优先使用 Level 1（零空间、O(1)），不可用时降级到 Level 2。Level 0 作为全盘兜底，即使 rew 进程崩溃也能从卷快照恢复。

#### 3.3.2 存储目录结构

```
.rew/
├── rew.db                    # SQLite 数据库（任务、变更、元数据）
├── objects/                  # 文件备份（内容寻址）
│   ├── a1/b2c3d4...          # 文件内容（clonefile 或压缩副本）
│   └── ...
├── config.json               # 本地配置
└── .rewscope                 # 作用域规则
```

#### 3.3.3 存储空间估算

| 场景 | CoW 模式（macOS） | 压缩模式（通用） |
|------|-----------------|----------------|
| 初始扫描 100 个文件 | ~0 MB（仅元数据） | ~20 MB |
| 每天增量（20 次 AI 任务） | ~2-5 MB | ~5-15 MB |
| 一个月累计 | ~60-150 MB | ~150-450 MB |
| 自动清理后（保留 30 天） | ≤ 150 MB | ≤ 450 MB |

#### 3.3.4 自动清理策略

- 默认保留 30 天
- 单项目存储上限 2 GB（可配置）
- 优先清理最老的、标记为「已确认」的快照
- 从未回退过的任务记录优先清理
- 大文件（>10 MB）保留时间减半

### 3.4 数据模型

```
Task（任务 = 一次用户 prompt）
├── id: string (nanoid)
├── prompt: string              # 用户原始 prompt 文本（来自 hook）
├── tool: string                # AI 工具名（Claude Code / Cursor / ...）
├── startedAt: timestamp
├── completedAt: timestamp
├── status: active | rolled-back | partial-rolled-back
├── summary: string             # AI 生成的变更摘要
├── riskLevel: low | medium | high
│
└── changes: Change[]           # 本次任务的所有文件变更
    ├── filePath: string
    ├── type: created | modified | deleted | renamed
    ├── oldHash: string         # 变更前的内容哈希（指向 objects/）
    ├── newHash: string         # 变更后的内容哈希
    ├── diff: string            # unified diff（文本文件）
    ├── linesAdded: number
    └── linesRemoved: number
```

### 3.5 作用域规则（.rewscope）

```yaml
# 允许 AI 操作的范围
allow:
  - ./**                        # 项目目录内全部允许

# 禁止 AI 操作的范围（触发 PreToolUse 拦截）
deny:
  - ~/Desktop/**
  - ~/Documents/**
  - ~/Downloads/**
  - ~/.ssh/**
  - ~/.aws/**
  - /**/.env
  - /**/.env.*

# 告警规则
alert:
  - pattern: "rm -rf"
    action: block               # 直接阻止
  - pattern: "rm -r"
    max_files: 20
    action: confirm             # 超过阈值告警
  - pattern: "> /dev/"
    action: block
  - bulk_delete:
    threshold: 50               # 10 秒内删除超过 50 个文件
    window_seconds: 10
    action: block
```

### 3.6 性能设计

核心原则：**同步 hook 只做最轻的操作，所有重活推到异步。**

| Hook | 同步/异步 | 操作 | 耗时 |
|------|---------|------|------|
| UserPromptSubmit | 同步 | 记录 prompt + 标记任务开始 | ~2ms |
| PreToolUse | 同步 | scope 检查 + clonefile 备份 | ~3ms |
| PostToolUse | 异步 | 记录变更到 DB | 0ms（不阻塞 AI） |
| Stop | 异步 | 打包任务 + 生成摘要 | 0ms（不阻塞 AI） |

20 次工具调用的总增加延迟：~62ms（AI 自身耗时 ~100 秒，rew 开销 0.06%）。

#### 3.6.1 CLI 启动优化

两种方案：

**方案 A：Daemon 模式（Node.js）**
- `rew start` 启动常驻后台进程
- hook 脚本通过 Unix domain socket 与 daemon 通信
- hook 脚本本身只是 `cat | nc -U /tmp/rew.sock`，启动 <5ms

**方案 B：编译型 CLI（Rust/Go）**
- 无需 daemon，每次 hook 直接调用 rew 二进制
- 冷启动 <5ms
- 推荐长期方案

---

## 四、产品形态

### 4.1 产品组成

```
rew 产品 = CLI + 桌面端 + AI 集成
           │      │        │
           │      │        ├── Hook 注入（多 AI 工具适配）
           │      │        ├── MCP Server（可选）
           │      │        └── AI 变更摘要 + 风险评估
           │      │
           │      └── 桌面端 GUI（类 Sourcetree）
           │          ├── 任务时间线
           │          ├── 文件变更详情
           │          ├── Diff 查看器
           │          └── 一键回退
           │
           └── CLI 命令行
               ├── rew install / uninstall
               ├── rew start / stop
               ├── rew list / show / undo
               └── rew hook <event>（内部命令）
```

### 4.2 CLI 命令设计

```bash
# 安装与初始化
rew install                      # 检测 AI 工具，注入 hook，生成 .rewscope
rew uninstall                    # 移除所有 hook

# 保护控制
rew start [path]                 # 启动文件保护（默认当前目录）
rew stop                         # 停止保护
rew status                       # 查看保护状态

# 查看与回退
rew list                         # 列出所有任务记录
rew show <id>                    # 查看某次任务的详细变更
rew diff <id>                    # 查看某次任务的 diff
rew undo [id]                    # 回退到某次任务之前
rew undo --file <path>           # 只回退某个文件
rew restore <id>                 # 重新应用已回退的任务

# 桌面端
rew app                          # 启动桌面端 GUI

# 内部命令（hook 调用）
rew hook prompt                  # UserPromptSubmit 处理
rew hook pre-tool                # PreToolUse 处理
rew hook post-tool               # PostToolUse 处理
rew hook stop                    # Stop 处理
```

### 4.3 桌面端 GUI 设计（类 Sourcetree）

#### 4.3.1 设计理念

Sourcetree 让不会 Git 的人也能做版本管理。rew 桌面端做同样的事——让不会 Git 的人能清晰看到 AI 干了什么，一键回到任意时间点。

核心体验：**打开 → 看到时间线 → 点击某次任务 → 看到改了什么 → 点一下回退**。三步完成。

#### 4.3.2 界面结构

```
┌──────────────────────────────────────────────────────────────┐
│  rew — AI 操作时间线                                    ─ □ ×  │
├──────────┬───────────────────────────────────────────────────┤
│          │                                                   │
│ 项目列表  │              任务详情区                             │
│          │                                                   │
│ ▶ my-app │  ┌─────────────────────────────────────────────┐  │
│   blog   │  │  #7  重构 auth 模块              3 分钟前     │  │
│   tools  │  │  🤖 Claude Code                             │  │
│          │  │  📝 "帮我重构一下 auth 模块"                  │  │
│          │  │                                             │  │
│          │  │  AI 摘要：将认证逻辑从 controller 抽离到      │  │
│          │  │  独立的 auth middleware，添加 JWT 验证...     │  │
│          │  │                                             │  │
│          │  │  12 个文件  +180 行  -95 行    风险：低 ✅     │  │
│          │  │                                             │  │
│          │  │  ┌─────────────────────────────────────┐    │  │
│          │  │  │  文件变更列表                         │    │  │
│──────────│  │  │                                     │    │  │
│          │  │  │  ✚ src/middleware/auth.js    +45     │    │  │
│ 时间线    │  │  │  ✚ src/middleware/jwt.js     +38     │    │  │
│          │  │  │  ✎ src/app.js               +8 -3   │    │  │
│ ● #7     │  │  │  ✎ src/routes/users.js      +12 -8  │    │  │
│ 3分钟前   │  │  │  ✎ src/routes/orders.js     +10 -6  │    │  │
│ 重构auth  │  │  │  ✎ src/routes/products.js   +8 -5   │    │  │
│          │  │  │  ✎ ...（6 more files）               │    │  │
│ ● #6     │  │  │  ✖ src/utils/old-auth.js    -52     │    │  │
│ 20分钟前  │  │  │  ✖ src/utils/old-jwt.js     -28     │    │  │
│ 添加注册  │  │  └─────────────────────────────────────┘    │  │
│          │  │                                             │  │
│ ● #5     │  │  [⬅ 回退到此任务之前]  [📋 查看完整 Diff]     │  │
│ 1小时前   │  └─────────────────────────────────────────────┘  │
│ 修复bug  │                                                   │
│          │  ┌─────────────────────────────────────────────┐  │
│ ● #4     │  │              Diff 查看器                     │  │
│ ● #3     │  │  src/app.js                                 │  │
│ ● #2     │  │  ─────────────────────────────               │  │
│ ● #1     │  │  @@ -12,5 +12,8 @@                          │  │
│          │  │  const express = require('express');         │  │
│          │  │  + const { authMiddleware } = require('./mi… │  │
│          │  │  + const { jwtVerify } = require('./middlew… │  │
│          │  │                                             │  │
│          │  │  app.use('/api', router);                    │  │
│          │  │  + app.use('/api', authMiddleware);          │  │
│          │  └─────────────────────────────────────────────┘  │
└──────────┴───────────────────────────────────────────────────┘
```

#### 4.3.3 核心功能

**1. 任务时间线（左侧栏）**

- 按时间倒序展示所有 AI 任务
- 每条显示：任务编号、时间、用户 prompt 摘要、AI 工具图标
- 颜色标识状态：绿色=正常、黄色=有风险、灰色=已回退
- 支持按项目筛选（多项目管理）
- 支持按日期范围、AI 工具、风险等级筛选

**2. 任务详情区（右上）**

- 用户原始 prompt
- AI 生成的变更摘要
- 统计信息：文件数、增删行数、风险评估
- 文件变更列表（创建/修改/删除分类显示）
- 点击文件跳转到 Diff 查看器

**3. Diff 查看器（右下）**

- 并排或统一 diff 视图（可切换）
- 语法高亮
- 支持逐文件浏览
- 二进制文件显示文件类型和大小变化

**4. 操作按钮**

- **回退到此任务之前**：撤销该任务及之后的所有变更
- **只回退这个任务**：撤销单个任务，保留后续变更
- **只回退某个文件**：选择性恢复
- **标记为安全**：确认该任务无问题，降低自动清理优先级

**5. 系统托盘**

- 最小化到系统托盘常驻运行
- 托盘图标显示保护状态（绿色=正在保护、灰色=未启动）
- 实时通知：AI 操作被拦截时弹出通知
- 右键菜单：打开主界面、暂停/恢复保护、退出

#### 4.3.4 技术选型

| 组件 | 技术 | 理由 |
|------|------|------|
| 桌面端框架 | Tauri 2.0 | Rust 后端 + WebView 前端，安装包 <10MB，内存占用低 |
| 前端 UI | React + Tailwind CSS | 生态好，组件丰富 |
| Diff 渲染 | react-diff-viewer / Monaco Editor | 成熟的 diff 可视化 |
| 数据通信 | Tauri IPC + SQLite | 本地数据，无需网络 |
| 系统托盘 | Tauri tray API | 原生支持 macOS / Windows / Linux |

Tauri 相比 Electron 的优势：安装包 ~8MB vs ~150MB，内存 ~30MB vs ~200MB。对于需要常驻后台的桌面应用非常关键。

---

## 五、AI 集成

### 5.1 AI 变更摘要

每次任务完成（Stop hook），异步调用本地或云端 LLM 生成：

```
输入：
- 用户 prompt："帮我重构一下 auth 模块"
- 文件变更列表 + diff 摘要

输出：
- 变更摘要："将认证逻辑从 controller 抽离到独立的 auth middleware，
  添加了 JWT 验证中间件，修改了 12 个路由文件注入认证检查"
- 风险评估：LOW
- 风险原因："所有改动在项目目录内，无敏感文件修改"
```

摘要显示在桌面端的任务详情区，用户不需要看 diff 就能知道 AI 干了什么。

### 5.2 意图 vs 行为比对

有了 UserPromptSubmit hook，rew 拿到了用户的原始 prompt。可以在 PreToolUse 阶段做快速比对：

```
用户 prompt："清空测试数据"
AI 正在执行：rm -rf ~/Desktop/2024年工作/*

规则引擎判断（不需要 LLM，<0.1ms）：
- 操作目标 ~/Desktop/ 不在项目目录内 → 越界
- 触发 .rewscope deny 规则 → 阻止

高级判断（可选，LLM）：
- prompt 意图：清空测试数据
- 实际操作：删除桌面个人文件
- 判定：明显不匹配 → HIGH RISK
```

### 5.3 智能回退建议

用户在桌面端点「回退」时，AI 分析依赖关系：

```
用户要回退 #7（重构 auth）

AI 分析：
- #8（添加用户注册）依赖 #7 新建的 auth middleware
- 如果只回退 #7，#8 的注册功能会报错

建议：
  [回退 #7 和 #8]  [只回退 #7（可能导致错误）]  [取消]
```

---

## 六、核心流程

### 6.1 安装流程

```
用户执行 rew install
    │
    ├── 1. 检测系统环境
    │   ├── 操作系统（macOS / Linux / Windows）
    │   ├── 文件系统类型（APFS / btrfs / ext4 / NTFS）
    │   └── 选择最优存储策略（CoW / 压缩复制）
    │
    ├── 2. 检测已安装的 AI 工具
    │   ├── ~/.claude/        → Claude Code
    │   ├── ~/.cursor/        → Cursor
    │   ├── .github/hooks/    → GitHub Copilot
    │   └── 其他工具检测
    │
    ├── 3. 为每个工具注入 hook 配置
    │   ├── Claude Code → .claude/settings.json
    │   ├── Cursor      → .cursor/hooks.json
    │   └── Copilot     → .github/hooks/rew.json
    │
    ├── 4. 生成 .rewscope 规则文件
    │
    ├── 5. 初始化 .rew/ 存储目录
    │
    └── 6. 启动 daemon 进程 + 文件监听
```

### 6.2 保护流程（有 Hook）

```
用户输入 prompt："帮我加一个登录功能"
    │
    ▼ UserPromptSubmit hook
    rew：创建 APFS 快照 + 记录 prompt + 开始新任务
    │
    ▼ AI 决定写文件：Write src/login.js
    │
    ▼ PreToolUse hook
    rew：检查 src/login.js 在 allow 范围内 ✓
    rew：文件不存在（新建），无需备份
    rew：exit 0，允许执行
    │
    ▼ AI 写入文件完成
    │
    ▼ PostToolUse hook（异步）
    rew：记录 {type: created, path: src/login.js, hash: xxx}
    │
    ▼ AI 决定编辑文件：Edit src/app.js
    │
    ▼ PreToolUse hook
    rew：检查 src/app.js 在 allow 范围内 ✓
    rew：clonefile 备份 src/app.js → .rew/objects/xxx（<1ms）
    rew：exit 0，允许执行
    │
    ▼ ...重复 N 次...
    │
    ▼ AI 决定执行命令：Bash "rm -rf ~/Desktop/*"
    │
    ▼ PreToolUse hook
    rew：检查 ~/Desktop/* 匹配 deny 规则 ✗
    rew：stderr "rew: 操作被拦截，~/Desktop/ 不在允许范围内"
    rew：exit 2，阻止执行 ❌
    │
    ▼ AI 收到拒绝，调整行为
    │
    ▼ AI 完成响应
    │
    ▼ Stop hook（异步）
    rew：打包任务 #7 = {prompt, 19 个变更, 1 次拦截}
    rew：生成 AI 摘要
    rew：通知桌面端更新时间线
```

### 6.3 回退流程

```
用户在桌面端点击「回退到 #7 之前」
    │
    ├── 解析任务 #7 的所有变更
    │
    ├── 对每个变更执行逆操作：
    │   ├── created 文件 → 删除
    │   ├── modified 文件 → 从 .rew/objects/ 恢复旧版本
    │   ├── deleted 文件 → 从 .rew/objects/ 恢复
    │   └── renamed 文件 → 重命名回去
    │
    ├── 标记任务 #7 状态为 rolled-back
    │
    └── 桌面端时间线刷新（#7 变灰）
```

---

## 七、开发路线图

### Phase 0：CLI MVP（2 周）

**目标**：跑通核心流程——备份 + 恢复。

- [ ] 项目初始化（TypeScript + Node.js）
- [ ] `rew start` 启动文件监听（chokidar）
- [ ] 文件变更检测 + debounce 聚合
- [ ] 文件备份（优先 clonefile，降级 copy + zstd）
- [ ] `rew list` 展示变更时间线
- [ ] `rew undo [id]` 回退变更
- [ ] `rew diff [id]` 查看 diff
- [ ] `.rewignore` 排除规则
- [ ] 自动清理（保留 N 天 / 上限 N GB）

**交付物**：一个能用的 CLI 工具，`npm install -g rew && rew start`。

### Phase 1：Hook 集成（2 周）

**目标**：从被动监听升级到主动保护。

- [ ] Claude Code hook 注入（PreToolUse / PostToolUse / UserPromptSubmit / Stop）
- [ ] Cursor hook 注入（beforeShellExecution / afterFileEdit / beforeSubmitPrompt / stop）
- [ ] GitHub Copilot hook 注入
- [ ] `rew install` 自动检测 + 注入
- [ ] `.rewscope` 作用域规则引擎
- [ ] PreToolUse 拦截（exit 2 阻止越界操作）
- [ ] prompt 文本记录 + 任务边界精确切分
- [ ] daemon 模式（Unix domain socket 通信）

**交付物**：AI 工具每次操作自动被 rew 保护，越界操作被拦截。

### Phase 2：桌面端 v1（3 周）

**目标**：让非技术用户也能使用。类 Sourcetree 的可视化。

- [ ] Tauri 2.0 + React 项目搭建
- [ ] 左侧任务时间线
- [ ] 右侧任务详情（prompt + 文件列表 + 统计）
- [ ] Diff 查看器（语法高亮、并排/统一切换）
- [ ] 一键回退（整个任务 / 单个文件）
- [ ] 系统托盘常驻 + 状态图标
- [ ] 拦截通知弹窗
- [ ] 多项目管理

**交付物**：一个安装即用的桌面应用，拖入项目文件夹即开始保护。

### Phase 3：AI 集成（2 周）

**目标**：用 AI 理解 AI 的行为。

- [ ] AI 变更摘要生成（本地 LLM 或 API）
- [ ] 风险评估（规则引擎 + 可选 LLM）
- [ ] 意图 vs 行为比对
- [ ] 智能回退建议（依赖分析）
- [ ] 桌面端集成 AI 摘要展示

### Phase 4：生态扩展（持续）

- [ ] APFS 卷快照集成（tmutil）
- [ ] Windows VSS 支持
- [ ] 团队协作（共享 .rewscope）
- [ ] VSCode/Cursor 插件（编辑器内嵌面板）
- [ ] `rew cloud`（可选的云端备份）

---

## 八、技术栈

| 组件 | 技术 | 理由 |
|------|------|------|
| CLI | TypeScript (Node.js) → 长期迁移 Rust | MVP 快速迭代，长期追求启动速度 |
| 文件监听 | chokidar | 成熟、跨平台、原生事件 |
| 数据库 | SQLite (better-sqlite3) | 嵌入式、零配置、单文件 |
| Diff 引擎 | diff-match-patch 或自实现 Myers | 轻量、无外部依赖 |
| 压缩 | zstd (fzstd) | 压缩率高、速度快 |
| 桌面端 | Tauri 2.0 + React + Tailwind | 轻量（<10MB）、原生托盘 |
| Diff 可视化 | Monaco Editor / react-diff-viewer | 语法高亮、成熟 |
| Hook 通信 | Unix domain socket / Named pipe | 极低延迟（<1ms） |
| CLI 解析 | commander.js | 成熟、用户量大 |

运行时依赖：chokidar, commander, better-sqlite3。保持极简。

---

## 九、竞品分析

| 维度 | rew | snaprevert | Git | Time Machine |
|------|-----|-----------|-----|-------------|
| 目标用户 | 所有 AI 用户 | 开发者 | 开发者 | 所有用户 |
| 学习成本 | 零 | 低 | 高 | 零 |
| 保护粒度 | 每次 AI 任务 | 文件变更 burst | 手动 commit | 每小时 |
| 操作前拦截 | ✅ Hook 拦截 | ❌ | ❌ | ❌ |
| 任务边界 | ✅ prompt→stop | ❌ debounce | 手动 | 时间 |
| 用户 prompt 关联 | ✅ 记录并展示 | ❌ | ❌（commit msg） | ❌ |
| AI 变更摘要 | ✅ AI 生成 | ❌ 自动标签 | 手动写 | ❌ |
| 桌面 GUI | ✅ 类 Sourcetree | ❌ CLI only | Sourcetree 等 | 系统内置 |
| 二进制文件 | ✅ clonefile | ❌ utf-8 only | ✅ | ✅ |
| 越界保护 | ✅ .rewscope | ❌ | ❌ | ❌ |
| 多 AI 工具适配 | ✅ 全部 | ✅ 全部 | N/A | N/A |
| 进程崩溃后恢复 | ✅ APFS 快照兜底 | ❌ 内存缓存丢失 | ✅ | ✅ |

**rew 的核心差异化：Hook 注入 + 任务级视图 + 桌面 GUI + 越界拦截。**

snaprevert 是一个好的验证——方向正确，但只覆盖了「看着」的部分。rew 同时覆盖「看着」「拦着」「兜底」三层。

---

## 十、风险与应对

| 风险 | 可能性 | 影响 | 应对策略 |
|------|-------|------|---------|
| AI 工具内置快照功能 | 高 | 核心竞争力被削弱 | rew 的价值在跨工具 + 拦截 + GUI，单工具的内置快照无法替代 |
| Hook API 变更 | 中 | 适配断裂 | 抽象适配层，一处修改适配所有版本 |
| 性能被质疑 | 低 | 用户不敢用 | 首次启动时显示性能基准测试结果 |
| 存储空间争议 | 中 | 用户卸载 | 默认 CoW（零空间）；积极的自动清理；设置面板显示存储占用 |
| macOS 安全限制 | 中 | tmutil 需要权限 | Level 1 (clonefile) 不需要权限；Level 0 (tmutil) 作为可选增强 |

---

## 十一、成功指标

### 11.1 北极星指标

**「至少有一个用户因为 rew 没有丢失自己的文件」**

### 11.2 量化指标

| 阶段 | 指标 | 目标 |
|------|------|------|
| MVP | 安装到首次保护 | < 60 秒 |
| MVP | 单次回退耗时 | < 1 秒 |
| Phase 1 | 拦截越界操作准确率 | > 99% |
| Phase 2 | 桌面端首次打开到理解界面 | < 30 秒 |
| Phase 2 | 周活用户留存 | > 40% |
| 6 个月 | GitHub Stars | > 1000 |

---

## 十二、传播策略

### 12.1 核心叙事

**「有人让 AI 清个数据，结果十三年工作文件全没了。所以我做了这个。」**

每一次 AI 安全事故都是 rew 的传播机会。不是消费灾难，而是提供解决方案。

### 12.2 渠道

| 渠道 | 内容形式 | 频率 |
|------|---------|------|
| GitHub README | 极简 Demo GIF + 30 秒上手 | 持续更新 |
| Hacker News | Show HN 帖子 | 发布时 |
| Reddit r/ClaudeCode r/cursor | 发布 + 评论区出现安全事故帖时回复 | 持续 |
| 微信公众号 | 长文（已有初稿） | 发布时 |
| Twitter/X | 30 秒演示视频 | 每周 |
| Product Hunt | 正式 Launch | Phase 2 完成后 |

### 12.3 病毒传播设计

- `rew undo` 成功后显示：「rew 刚刚帮你恢复了 12 个文件。Share: [链接]」
- 拦截越界操作后显示：「rew 阻止了一次可能的误操作。了解更多: [链接]」
- GitHub README 首行：「The undo button for AI-assisted work.」
