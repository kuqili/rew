# rew — AI 时代的文件安全网

rew 自动监控文件变更，检测异常操作（如误删、批量修改），并通过 macOS APFS 快照保护你的数据。轻松恢复误删、误改的文件。

## 安装

### 从源码构建

```bash
# 安装依赖
pnpm install

# 构建 CLI
cargo build -p rew-cli --release

# 构建 Tauri 桌面应用（含 .dmg）
pnpm tauri build
```

### 安装 CLI

```bash
# 将 rew 复制到 PATH
cp target/release/rew /usr/local/bin/

# 初始化配置
rew init
```

### 安装桌面应用

双击 `src-tauri/target/release/bundle/dmg/rew_0.1.0_aarch64.dmg` 安装。

## 使用

### CLI 命令

```bash
rew status          # 查看运行状态和快照列表
rew list            # 查看所有快照详情
rew restore         # 交互式选择恢复点
rew config show     # 查看当前配置
rew config add-dir ~/Projects  # 添加保护目录
rew pin <id>        # 标记快照为永久保留
rew install         # 安装 LaunchAgent（开机自启）
rew uninstall       # 移除 LaunchAgent
rew daemon          # 前台运行守护进程
```

### 开机自启

```bash
rew install    # 注册 LaunchAgent，登录后自动启动
rew uninstall  # 移除自启
```

### 桌面应用

- 系统托盘常驻，显示运行状态
- 点击托盘图标打开时间线界面
- 支持一键恢复、查看快照详情
- 首次启动引导选择保护目录

## 异常检测规则

| 规则 | 触发条件 | 级别 |
|------|----------|------|
| RULE-01 | 批量删除 > 20 文件 | HIGH |
| RULE-02 | 批量删除 5-20 文件 | MEDIUM |
| RULE-03 | 删除总大小 > 100MB | HIGH |
| RULE-04 | 批量修改 > 50 文件 | MEDIUM |
| RULE-05 | 监控根目录被删除 | CRITICAL |
| RULE-06 | 敏感配置文件（.env 等）被修改 | HIGH |
| RULE-07 | 非包管理器的大量修改 > 30 | MEDIUM |

## 配置文件

配置存储在 `~/.rew/config.toml`：

```toml
watch_dirs = ["~/Desktop", "~/Documents", "~/Downloads"]
ignore_patterns = ["**/node_modules/**", "**/.git/**", "**/target/**"]
```

## 快照保留策略

- 1 小时内：全部保留
- 1-24 小时：每小时保留 1 个
- 1-30 天：每天保留 1 个
- 超过 30 天：自动删除
- 异常快照：保留时间翻倍
- 已标记快照：永不删除

## 开发

```bash
# 运行测试
cargo test --workspace

# 开发模式启动 Tauri
pnpm tauri dev
```

## 系统要求

- macOS 11.0+
- APFS 文件系统（默认）
