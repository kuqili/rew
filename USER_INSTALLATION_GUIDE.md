# rew 用户安装指南

为最终用户提供的完整安装和配置指南。

## 📥 第一步：安装 rew 桌面应用

### 从 GitHub Releases 下载

1. 访问 [rew Releases](https://github.com/kuqili/rew/releases)
2. 找到最新版本，选择对应 Mac 型号的 DMG 文件：
   - **Apple Silicon (M1/M2/M3/M4)**：下载 `rew_*_aarch64.dmg`
   - **Intel Mac**：下载 `rew_*_x64.dmg`

### 安装应用

1. 打开下载的 `.dmg` 文件（双击）
2. 将 **rew** 应用拖到 **Applications** 文件夹
3. 等待复制完成

### Gatekeeper 安全提示

由于 rew 未进行 Apple 公证，首次启动时可能看到"无法验证开发者"的警告：

1. 打开 Finder，进入 **Applications** 文件夹
2. 找到 **rew** 应用
3. **右键点击** → 选择 **打开**
4. 在弹窗中点击 **打开** 按钮
5. 之后启动不再需要重复此操作

> **提示**：如果还是无法打开，可以在系统偏好设置 → 安全性与隐私 中允许应用运行。

---

## 🔧 第二步：启用 AI 工具集成

### 什么是 Hook？

Hook 是一种集成机制，让 rew 能够：
- 在 Claude Code、Cursor 等 AI 工具执行操作前进行权限检查
- 自动备份文件
- 记录所有 AI 操作
- 让你能够随时一键撤销

### 快速安装 Hook

#### 方法 A：自动安装（推荐）

1. **打开 rew 应用**
   - 从 Launchpad 或 Applications 文件夹启动 rew

2. **在应用中运行安装命令**
   - 打开应用菜单 → 选择 "Settings" 或 "Preferences"
   - 找到 "Install AI Tool Hooks" 选项
   - 点击 "Auto Install"

   或者从终端运行：
   ```bash
   /Applications/rew.app/Contents/MacOS/rew install
   ```

3. **验证安装成功**
   ```bash
   grep "rew hook" ~/.claude/settings.json
   # 应该能看到 4 个 hook 命令行
   ```

#### 方法 B：手动安装

如果自动安装失败，可以手动编辑 Claude Code 的配置文件：

1. **备份现有配置**
   ```bash
   cp ~/.claude/settings.json ~/.claude/settings.json.backup
   ```

2. **获取 rew 二进制路径**
   ```bash
   # 从 DMG 安装的情况下：
   /Applications/rew.app/Contents/MacOS/rew --version
   ```

3. **编辑配置文件**
   ```bash
   # 使用你喜欢的编辑器打开
   nano ~/.claude/settings.json
   ```

4. **添加 Hook 配置**

   在 JSON 中找到或创建 `"hooks"` 部分，添加以下内容：

   ```json
   {
     "hooks": {
       "UserPromptSubmit": [
         {
           "matcher": "",
           "hooks": [
             {
               "type": "command",
               "command": "/Applications/rew.app/Contents/MacOS/rew hook prompt"
             }
           ]
         }
       ],
       "PreToolUse": [
         {
           "matcher": "",
           "hooks": [
             {
               "type": "command",
               "command": "/Applications/rew.app/Contents/MacOS/rew hook pre-tool"
             }
           ]
         }
       ],
       "PostToolUse": [
         {
           "matcher": "",
           "hooks": [
             {
               "type": "command",
               "command": "/Applications/rew.app/Contents/MacOS/rew hook post-tool"
             }
           ]
         }
       ],
       "Stop": [
         {
           "matcher": "",
           "hooks": [
             {
               "type": "command",
               "command": "/Applications/rew.app/Contents/MacOS/rew hook stop"
             }
           ]
         }
       ]
     }
   }
   ```

5. **验证 JSON 格式正确**
   ```bash
   python3 -m json.tool ~/.claude/settings.json > /dev/null
   echo $?  # 应该输出 0
   ```

---

## 🚀 第三步：启动后台守护

Hook 安装完成后，需要启动 rew 的后台守护进程来记录 AI 操作。

### 方法 A：通过 LaunchAgent 自动启动（推荐）

运行安装命令时会自动配置 LaunchAgent，每次登录时自动启动 rew 守护：

```bash
# 验证 LaunchAgent 已安装
launchctl list | grep rew
# 应该看到 "com.rew.daemon"

# 手动启动（如需要立即启动）
launchctl start com.rew.daemon

# 查看运行状态
launchctl list com.rew.daemon
```

### 方法 B：手动启动

需要时可以手动启动守护进程：

```bash
/Applications/rew.app/Contents/MacOS/rew daemon
```

---

## ✅ 验证安装

完成以上步骤后，验证一切是否正常工作：

### 1. 检查 Hook 配置

```bash
cat ~/.claude/settings.json | jq '.hooks'
# 应该看到 4 个 hook 定义
```

### 2. 检查后台守护

```bash
pgrep -f "rew daemon"
# 应该看到进程 ID

# 如果没有输出，说明守护没有运行
# 可以手动启动或检查 LaunchAgent
```

### 3. 检查数据库

```bash
sqlite3 ~/.rew/snapshots.db "SELECT COUNT(*) FROM tasks;"
# 应该看到任务数量

# 查看任务详情
sqlite3 ~/.rew/snapshots.db "SELECT id, tool, prompt FROM tasks LIMIT 5;"
```

### 4. 使用命令行工具验证

```bash
# 列出所有任务
/Applications/rew.app/Contents/MacOS/rew list

# 查看特定任务
/Applications/rew.app/Contents/MacOS/rew show <task-id>
```

---

## 🧪 端到端测试

现在让我们测试整个流程是否正常工作：

### 第 1 步：重启 Claude Code

必须完全关闭并重新启动 Claude Code，以加载新的 Hook 配置。

### 第 2 步：创建测试文件

在 Claude Code 中开启新对话，使用以下提示词：

```
Create a new file called test.md with this content:
# Test File
This was created by Claude Code with rew hooks enabled.
```

让 Claude Code 执行 Write 操作。

### 第 3 步：检查 rew 应用

1. **打开 rew 桌面应用**
2. **切换到 "AI 任务" 选项卡**
3. **看是否有新任务出现**

你应该看到：
- ✓ 一个新的 task 条目
- ✓ 显示 "Claude Code" 标签
- ✓ 显示你的提示文本（截断显示）
- ✓ 显示 "1 文件" 
- ✓ 当前时间戳

### 第 4 步：查看任务详情

1. **点击任务**进入详情视图
2. 你应该看到：
   - ✓ 左侧：修改的文件列表（test.md）
   - ✓ 文件类型标签："A"（新增）
   - ✓ 右侧：Diff 预览，显示新增的内容

### 第 5 步：测试撤销功能

1. **点击文件旁的 "↩ 读档" 按钮**
2. **在弹窗中确认**
3. **验证文件被删除**

检查 test.md 是否从你的文件系统中消失。

---

## 🔒 权限和作用域

### .rewscope 文件

rew 使用 `.rewscope` 文件来定义哪些文件和命令 AI 工具可以操作：

**自动生成的默认规则：**

```yaml
allow:
  - "./**"              # 允许当前目录中的所有操作

deny:
  - "~/.ssh/**"         # 不允许访问 SSH 密钥
  - "~/.aws/**"         # 不允许访问 AWS 凭证
  - "/**/.env"          # 不允许修改 .env 文件
  - "/**/.env.*"        # 不允许修改任何 .env.* 文件
```

**自定义作用域：**

如果需要调整规则，编辑项目根目录的 `.rewscope` 文件：

```yaml
allow:
  - "src/**"            # 只允许 src 目录
  - "tests/**"

deny:
  - "src/config/**"     # 不允许修改配置
```

> **提示**：每个项目可以有自己的 `.rewscope` 文件来定制规则。

---

## 🐛 故障排除

### 问题 1：Hook 没有被调用

**症状**：在 Claude Code 执行操作后，任务没有出现在 rew 应用中

**排查步骤：**

```bash
# 1. 检查 settings.json 是否包含 rew hook
grep "rew hook" ~/.claude/settings.json || echo "❌ Hook 不存在"

# 2. 验证 JSON 格式
python3 -m json.tool ~/.claude/settings.json > /dev/null || echo "❌ JSON 格式错误"

# 3. 检查 rew 二进制是否可访问
/Applications/rew.app/Contents/MacOS/rew --version

# 4. 检查守护进程是否运行
pgrep -f "rew daemon" || echo "❌ 守护进程未运行"
```

**解决方案：**

1. 完全关闭 Claude Code（Command + Q）
2. 重新运行安装：
   ```bash
   /Applications/rew.app/Contents/MacOS/rew install
   ```
3. 重新启动 Claude Code

### 问题 2：任务未显示在 rew 应用

**症状**：Hook 工作但任务未出现在 UI 中

**排查步骤：**

```bash
# 1. 检查数据库中是否有任务
sqlite3 ~/.rew/snapshots.db "SELECT COUNT(*) FROM tasks;"

# 2. 检查守护进程状态
pgrep -f "rew daemon"

# 3. 查看最近的任务
sqlite3 ~/.rew/snapshots.db "SELECT id, started_at FROM tasks ORDER BY started_at DESC LIMIT 1;"
```

**解决方案：**

1. 确保守护进程在运行：
   ```bash
   launchctl start com.rew.daemon
   ```
2. 或手动启动：
   ```bash
   /Applications/rew.app/Contents/MacOS/rew daemon
   ```
3. 刷新 rew 应用（关闭重新打开）

### 问题 3：看到"权限被拒绝"（Exit code 2）

**症状**：Claude Code 操作被 rew 阻止

**原因**：`.rewscope` 规则阻止了操作

**解决方案：**

```bash
# 1. 检查当前规则
cat .rewscope

# 2. 放宽规则（示例）
# 编辑 .rewscope 并允许更多路径
allow:
  - "./**"           # 允许所有
```

### 问题 4：不想启用 Hook

如果不想使用 Hook 功能，可以禁用它：

```bash
# 移除 Hook 配置
/Applications/rew.app/Contents/MacOS/rew uninstall

# 或手动从 ~/.claude/settings.json 删除 hooks 部分
```

rew 仍然可以进行手动文件监听和备份，但不会自动跟踪 AI 工具的操作。

---

## 📚 更多资源

- **[IMPLEMENTATION_CHECKLIST.md](./IMPLEMENTATION_CHECKLIST.md)** - 详细的验证检查表
- **[HOOK_ARCHITECTURE.md](./HOOK_ARCHITECTURE.md)** - Hook 系统技术细节
- **[QUICK_REFERENCE.md](./QUICK_REFERENCE.md)** - 数据模型快速参考
- **[README.md](./README.md)** - 项目概述

---

## 🆘 需要帮助？

### 常见问题

**Q：rew 会持续消耗电池吗？**
A：rew 使用 macOS 的 FSEvents 实时监听，类似于 Spotlight。能耗极低，对电池影响不大。

**Q：rew 会上传我的文件吗？**
A：不会。rew 完全离线运行，所有文件备份存储在本地 `~/.rew/objects/` 目录。

**Q：可以连接多台设备吗？**
A：目前 rew 是单机工具。备份存储在本地。如需多设备同步，可使用 iCloud Drive。

**Q：支持其他 AI 工具吗？**
A：目前支持 Claude Code 和 Cursor。GitHub Copilot 的支持正在开发中。

### 获取日志

遇到问题时，收集日志有助于诊断：

```bash
# LaunchAgent 日志
log stream --predicate 'process == "rew"' --level debug

# 应用日志（如有）
tail -f ~/.rew/logs/*.log

# 数据库查询
sqlite3 ~/.rew/snapshots.db ".dump" > ~/rew-db-dump.sql
```

### 反馈和问题

- 📧 发送邮件：[your-email]
- 🐛 报告 Bug：https://github.com/kuqili/rew/issues
- 💬 讨论功能：https://github.com/kuqili/rew/discussions

---

## 🎉 成功标志

当所有以下条件都满足时，说明安装成功：

- [ ] ✅ `rew install` 命令执行无错误
- [ ] ✅ Claude Code 设置文件包含 rew hook 命令
- [ ] ✅ rew 后台守护进程在运行（`pgrep -f "rew daemon"`）
- [ ] ✅ Claude Code 执行操作后任务出现在 rew 应用中
- [ ] ✅ 可以查看任务详情和文件变更
- [ ] ✅ "↩ 读档" 按钮成功恢复文件

完成以上所有步骤后，您已经完全启用了 rew 的保护功能！🚀

---

## 版本信息

- **rew 版本**：0.1.0
- **最后更新**：2026-04-09
- **支持的 macOS 版本**：11.0+
- **支持的文件系统**：APFS（macOS 默认）
