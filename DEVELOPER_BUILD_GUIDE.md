# rew 开发者构建指南

为想要从源代码构建 rew 或参与开发的开发者提供的指南。

## 📋 系统要求

- **macOS**: 11.0 或更高版本
- **文件系统**: APFS（macOS 默认）
- **Rust**: 1.70 或更高版本（通过 rustup 安装）
- **Node.js**: 18+ 和 pnpm 9+
- **Xcode Command Line Tools**: 必需

## 🚀 快速开始

### 1. 安装依赖

```bash
# 安装 Rust（如未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# 添加 macOS targets（arm64 和 x86_64）
rustup target add aarch64-apple-darwin x86_64-apple-darwin

# 安装 Xcode Command Line Tools
xcode-select --install

# 安装 Node.js（如未安装，推荐使用 nvm）
# 访问 https://nodejs.org 或使用 brew install node

# 安装 pnpm
npm install -g pnpm@9
```

### 2. 克隆项目

```bash
git clone https://github.com/kuqili/rew.git
cd rew
```

### 3. 安装前端依赖

```bash
cd gui
pnpm install
cd ..
```

### 4. 开发模式（热重载）

```bash
# 启动 Tauri 开发服务器
# 自动启动 Vite dev 服务器 + Tauri app
pnpm dev
```

这将：
1. 启动 `gui/` 中的 Vite 开发服务器（通常在 `localhost:5173`）
2. 编译 Rust 后端
3. 启动 rew 桌面应用（连接到 Vite dev 服务器）
4. 监听文件变化并热重载

### 5. 构建发布版本

```bash
# 构建优化的 Tauri DMG 应用
pnpm build

# 生成的 DMG 文件位置：
# src-tauri/target/release/bundle/dmg/
```

---

## 🏗️ 项目结构

```
rew/
├── Cargo.toml                 # Rust workspace 根配置
│
├── crates/                    # Rust crates（可复用的库）
│   ├── rew-core/              # 核心库（无 Tauri 依赖）
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs         # 库入口点
│   │       ├── types.rs       # 核心数据结构
│   │       ├── db.rs          # SQLite 数据库层
│   │       ├── objects.rs     # 内容寻址存储
│   │       ├── scanner.rs     # 文件系统扫描器
│   │       ├── pipeline.rs    # FSEvents 处理管道
│   │       ├── restore.rs     # 撤销/恢复引擎
│   │       ├── scope.rs       # .rewscope 规则引擎
│   │       ├── backup/        # clonefile 备份
│   │       ├── detector/      # 异常检测规则
│   │       ├── error.rs       # 错误类型
│   │       └── config.rs      # 配置管理
│   │
│   └── rew-cli/               # 命令行工具
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs        # CLI 入口点
│           └── commands/      # 子命令
│               ├── hook.rs    # 4 个 hook 处理函数
│               ├── install.rs # Hook 注入 + LaunchAgent
│               ├── daemon.rs  # 后台守护启动
│               ├── list.rs    # 任务列表
│               └── ...
│
├── src-tauri/                 # Tauri 应用后端
│   ├── tauri.conf.json        # Tauri 配置（窗口、权限等）
│   └── src/
│       ├── main.rs            # Tauri 应用入口
│       ├── lib.rs             # 库入口点
│       ├── commands.rs        # IPC 命令处理
│       ├── daemon.rs          # 后台守护进程
│       ├── state.rs           # 共享应用状态
│       └── tray.rs            # 系统托盘菜单
│
├── gui/                       # React 前端
│   ├── src/
│   │   ├── main.tsx           # React 应用入口
│   │   ├── App.tsx            # 主应用组件
│   │   ├── components/        # UI 组件
│   │   │   ├── TaskTimeline.tsx   # 任务列表
│   │   │   ├── TaskDetail.tsx     # 任务详情
│   │   │   ├── DiffViewer.tsx     # Diff 查看器
│   │   │   └── ...
│   │   ├── hooks/             # React hooks
│   │   │   ├── useTasks.ts    # 任务数据获取
│   │   │   ├── useScanProgress.ts
│   │   │   └── ...
│   │   ├── lib/               # 工具函数
│   │   │   ├── tauri.ts       # Tauri IPC 封装
│   │   │   ├── format.ts      # 格式化函数
│   │   │   └── ...
│   │   └── styles/
│   │       └── globals.css
│   ├── tailwind.config.js     # Tailwind 配置
│   ├── vite.config.ts         # Vite 构建配置
│   ├── tsconfig.json
│   └── package.json
│
└── docs/                      # 文档
    ├── CLAUDE_CODE_HOOK_INTEGRATION.md
    ├── INSTALLATION_GUIDE.md
    └── ...
```

---

## 🔨 常见开发任务

### 添加新的 CLI 命令

1. **在 `crates/rew-cli/src/commands/` 中创建新文件**

```rust
// my_command.rs
pub fn handle_my_command() -> RewResult<()> {
    println!("执行我的命令");
    Ok(())
}
```

2. **在 `crates/rew-cli/src/commands/mod.rs` 中注册**

```rust
pub mod my_command;
pub use my_command::handle_my_command;
```

3. **在 `crates/rew-cli/src/main.rs` 中添加 CLI 子命令**

```rust
match args[1].as_str() {
    "my-command" => handle_my_command()?,
    // ...
}
```

### 修改数据库模式

1. **编辑 `crates/rew-core/src/db.rs` 中的初始化函数**
2. **添加新的 CREATE TABLE 语句**
3. **更新相关的查询函数**

示例：

```rust
// 在 db.rs 中的 initialize() 函数
pub fn initialize(&self) -> RewResult<()> {
    let conn = self.open()?;
    
    conn.execute(
        "CREATE TABLE IF NOT EXISTS my_table (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            created_at TIMESTAMP
        )",
        [],
    )?;
    
    Ok(())
}
```

### 添加新的前端组件

1. **创建新的 `.tsx` 文件**

```typescript
// components/MyComponent.tsx
export default function MyComponent() {
  return <div>My Component</div>;
}
```

2. **在需要的地方导入使用**

```typescript
import MyComponent from './components/MyComponent';
```

3. **使用 Tauri IPC 获取数据**

```typescript
import { invoke } from '@tauri-apps/api/core';

const data = await invoke('my_command', { arg1: 'value' });
```

### 测试

```bash
# 运行 Rust 单元测试
cargo test

# 运行特定 crate 的测试
cargo test -p rew-core

# 运行集成测试
cargo test --test '*'

# 运行 TypeScript/JavaScript 检查
cd gui && pnpm type-check
```

---

## 📦 构建不同架构的发布版本

### 构建 ARM64（Apple Silicon）

```bash
# 使用 tauri-cli 为 arm64 构建
pnpm tauri build -- --target aarch64-apple-darwin
```

### 构建 x86_64（Intel）

```bash
# 为 Intel Mac 构建
pnpm tauri build -- --target x86_64-apple-darwin
```

### 构建通用二进制（需要在 macOS 12+）

```bash
# 创建通用二进制（同时包含 arm64 和 x86_64）
# 这需要修改 tauri.conf.json 或使用 lipo 工具
```

### 输出文件

构建完成后，DMG 文件位置：

```
src-tauri/target/release/bundle/dmg/
├── rew_0.1.0_aarch64.dmg      # Apple Silicon
└── rew_0.1.0_x64.dmg           # Intel
```

---

## 🔗 IPC 命令参考

### 添加新的 IPC 命令

1. **在 `src-tauri/src/commands.rs` 中定义**

```rust
use tauri::State;
use rew_core::db::Database;

#[tauri::command]
pub async fn my_new_command(task_id: String, state: State<'_, AppState>) -> Result<String, String> {
    // 实现命令逻辑
    Ok("Success".to_string())
}
```

2. **在 `src-tauri/src/main.rs` 中注册**

```rust
.invoke_handler(tauri::generate_handler![
    commands::list_tasks,
    commands::get_task,
    commands::my_new_command,  // 添加这里
    // ...
])
```

3. **从前端调用**

```typescript
const result = await invoke('my_new_command', { task_id: '123' });
```

---

## 🐛 调试

### 启用详细日志

```bash
# 设置日志级别
RUST_LOG=debug pnpm dev

# 或在 Rust 代码中使用 env_logger
use log::{info, debug, error};
info!("这是一条信息日志");
```

### 使用 VS Code 调试

安装以下扩展：
- CodeLLDB（Rust 调试）
- Debugger for Chrome（前端调试）

配置 `.vscode/launch.json`：

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "lldb",
      "request": "launch",
      "name": "Debug Rust",
      "cargo": {
        "args": ["build", "-p", "rew-cli"],
        "filter": {
          "name": "rew",
          "kind": "bin"
        }
      }
    }
  ]
}
```

### 查看数据库内容

```bash
# 打开 SQLite 数据库
sqlite3 ~/.rew/snapshots.db

# 常用查询
SELECT * FROM tasks;
SELECT * FROM changes WHERE task_id = 't0409...';
SELECT COUNT(*) FROM objects;
```

---

## 📝 代码风格指南

### Rust

- 遵循 [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- 使用 `rustfmt` 格式化代码：`cargo fmt`
- 使用 `clippy` 检查潜在问题：`cargo clippy`

```rust
// ✅ Good
pub fn process_file(path: &Path) -> RewResult<()> {
    // 实现
}

// ❌ Avoid
fn processFile(path: &Path) -> RewResult<()> {
    // ...
}
```

### TypeScript/React

- 遵循 [React 最佳实践](https://react.dev)
- 使用 `const` 声明（避免 `var`）
- 使用 `interface` 定义类型（而不是 `type`）
- 添加适当的注释

```typescript
// ✅ Good
interface TaskInfo {
  id: string;
  prompt: string | null;
  started_at: string;
}

const [tasks, setTasks] = useState<TaskInfo[]>([]);

// ❌ Avoid
const [tasks, setTasks] = useState([]);  // 没有类型提示
```

---

## 🤝 贡献指南

### 提交 PR 前的检查清单

- [ ] 代码通过 `cargo fmt` 和 `cargo clippy`
- [ ] 所有测试通过：`cargo test`
- [ ] 文档已更新（如有 API 变更）
- [ ] 提交信息清晰且遵循 [Conventional Commits](https://www.conventionalcommits.org/)

### 提交信息格式

```
feat: Add new feature
fix: Fix bug
docs: Update documentation
test: Add tests
refactor: Refactor code
perf: Improve performance
ci: Update CI/CD
```

---

## 📚 资源

- [Rust 文档](https://doc.rust-lang.org/)
- [Tauri 文档](https://tauri.app/)
- [React 文档](https://react.dev/)
- [SQLite 文档](https://www.sqlite.org/docs.html)

---

## 🆘 常见问题

**Q: 构建时出现链接错误**
A: 确保已安装 Xcode Command Line Tools：`xcode-select --install`

**Q: Tauri dev 启动很慢**
A: 这是正常的（首次编译可能需要 5-10 分钟）。后续的热重载会快得多。

**Q: 如何清理构建产物？**
A: 运行 `cargo clean` 清理所有构建产物，然后重新构建。

---

版本：0.1.0  
最后更新：2026-04-09
