# rew 文档导航地图

完整的 rew 项目文档索引和导航指南。选择最适合你的文档开始！

## 🎯 按角色选择文档

### 👤 我是普通用户，想要使用 rew

**首先阅读这些（按顺序）：**

1. **[README.md](./README.md)** — 项目概述（5 分钟）
   - rew 是什么
   - 核心功能介绍
   - 系统要求

2. **[USER_INSTALLATION_GUIDE.md](./USER_INSTALLATION_GUIDE.md)** — 完整安装指南（20 分钟）
   - 从 GitHub Releases 下载 DMG
   - 安装到 Applications 文件夹
   - 设置 AI 工具 Hook
   - Gatekeeper 安全提示处理
   - **完整的故障排除章节**

3. **[IMPLEMENTATION_CHECKLIST.md](./IMPLEMENTATION_CHECKLIST.md)** — 验证清单（10 分钟）
   - 逐步验证安装
   - 手动测试步骤
   - 端到端测试流程

**遇到问题了？**

查看 [USER_INSTALLATION_GUIDE.md#🐛-故障排除](./USER_INSTALLATION_GUIDE.md#🐛-故障排除) 中的诊断和解决方案。

---

### 💻 我是开发者，想要从源代码构建 rew

**按这个顺序阅读：**

1. **[DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md)** — 从源代码构建（30 分钟）
   - 系统依赖安装
   - 快速开始命令
   - 项目结构说明
   - 常见开发任务示例
   - 测试和调试

2. **[README.md](./README.md)** — 项目概述
   - 理解 rew 的目标和功能

3. **[ARCHITECTURE_SUMMARY.md](./ARCHITECTURE_SUMMARY.md)** — 系统架构概览（15 分钟）
   - 组件交互
   - 数据流程
   - 状态管理

**想了解 Hook 系统的实现？**

继续阅读 [HOOK_ARCHITECTURE.md](#hook-系统)

---

### 🔧 我想理解 Claude Code Hook 系统

**技术深潜（按顺序）：**

1. **[QUICK_REFERENCE.md](./QUICK_REFERENCE.md)** — 快速参考（10 分钟）
   - 数据流图
   - 数据模型
   - 组件通信
   
2. **[HOOK_ARCHITECTURE.md](./HOOK_ARCHITECTURE.md)** — Hook 系统详解（25 分钟）
   - 四个 Hook 事件详解
   - 每个 Handler 的职责
   - 数据库操作
   - 权限检查流程

3. **[CLAUDE_CODE_HOOK_INTEGRATION.md](./CLAUDE_CODE_HOOK_INTEGRATION.md)** — 集成指南（20 分钟）
   - Hook 系统完整概览
   - 事件系统详解
   - 数据流程图表
   - 性能特征
   - 扩展建议

4. **[IMPLEMENTATION_CHECKLIST.md](./IMPLEMENTATION_CHECKLIST.md)** — 实现验证
   - 检查清单
   - 手动测试步骤

---

### 📊 我想了解系统架构

**架构深潜（按顺序）：**

1. **[REW_ARCHITECTURE.md](./REW_ARCHITECTURE.md)** — 完整架构（30 分钟）
   - 系统各层说明
   - 组件职责
   - 数据模型详解
   - 关键设计决策

2. **[ARCHITECTURE_SUMMARY.md](./ARCHITECTURE_SUMMARY.md)** — 架构概览（15 分钟）
   - 高层概览
   - 集成点
   - 状态管理

3. **[QUICK_REFERENCE.md](./QUICK_REFERENCE.md)** — 快速参考
   - 数据流图
   - 对象关系图
   - 数据库架构

---

### 🧪 我想为 rew 做贡献

**贡献者指南（按顺序）：**

1. **[DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md)** — 开发者构建指南
   - 项目设置
   - 代码风格
   - 贡献检查清单

2. **[REW_ARCHITECTURE.md](./REW_ARCHITECTURE.md)** — 系统架构
   - 了解项目结构
   - 理解各模块职责

3. **[HOOK_ARCHITECTURE.md](./HOOK_ARCHITECTURE.md)** — Hook 系统（如计划修改 Hook）
   - Hook 详细实现
   - 数据流程

4. **[COMPETITOR_HOOK_RESEARCH.md](./COMPETITOR_HOOK_RESEARCH.md)** — 竞争对手分析
   - 了解其他工具的设计
   - 发现改进机会

---

### 🔍 我想对比其他 AI 工具的 Hook 系统

**竞争分析（按顺序）：**

1. **[COMPETITOR_HOOK_RESEARCH.md](./COMPETITOR_HOOK_RESEARCH.md)** — 竞争对手分析（20 分钟）
   - Claude Code vs Cursor vs Copilot
   - Hook 事件对比
   - 配置格式对比
   - 未来扩展建议

---

## 📋 文档完整列表

| 文档 | 行数 | 类型 | 适合对象 | 阅读时间 |
|------|------|------|---------|---------|
| **README.md** | 104 | 概述 | 所有人 | 5 分钟 |
| **USER_INSTALLATION_GUIDE.md** | 472 | 用户指南 | 最终用户 | 20 分钟 |
| **DEVELOPER_BUILD_GUIDE.md** | 466 | 开发指南 | 开发者 | 30 分钟 |
| **IMPLEMENTATION_CHECKLIST.md** | 375 | 检查清单 | 开发者/测试 | 10 分钟 |
| **QUICK_REFERENCE.md** | 313 | 快速参考 | 开发者/架构师 | 10 分钟 |
| **HOOK_ARCHITECTURE.md** | 501 | 技术深潜 | 开发者/架构师 | 25 分钟 |
| **CLAUDE_CODE_HOOK_INTEGRATION.md** | 772 | 集成指南 | 开发者/贡献者 | 30 分钟 |
| **ARCHITECTURE_SUMMARY.md** | 563 | 架构说明 | 开发者/架构师 | 20 分钟 |
| **REW_ARCHITECTURE.md** | 614 | 完整架构 | 开发者/贡献者 | 30 分钟 |
| **COMPETITOR_HOOK_RESEARCH.md** | 362 | 竞争分析 | 产品/开发 | 20 分钟 |
| **DOCUMENTATION_MAP.md** | 此文件 | 导航 | 所有人 | 5 分钟 |

总计：**5,542 行** 完整文档

---

## 🗺️ 文档关系图

```
README.md (项目概述)
    ├─ USER_INSTALLATION_GUIDE.md (最终用户)
    │   ├─ IMPLEMENTATION_CHECKLIST.md (验证)
    │   └─ QUICK_REFERENCE.md (参考)
    │
    └─ DEVELOPER_BUILD_GUIDE.md (开发者)
        ├─ ARCHITECTURE_SUMMARY.md (架构概览)
        │   └─ REW_ARCHITECTURE.md (完整架构)
        │
        ├─ HOOK_ARCHITECTURE.md (Hook 系统)
        │   ├─ CLAUDE_CODE_HOOK_INTEGRATION.md (集成)
        │   ├─ COMPETITOR_HOOK_RESEARCH.md (竞争分析)
        │   └─ QUICK_REFERENCE.md (数据模型)
        │
        └─ IMPLEMENTATION_CHECKLIST.md (验证)
```

---

## 🔍 按主题查找文档

### 安装和设置

- **最终用户**：[USER_INSTALLATION_GUIDE.md](./USER_INSTALLATION_GUIDE.md)
- **从源代码构建**：[DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md)
- **验证步骤**：[IMPLEMENTATION_CHECKLIST.md](./IMPLEMENTATION_CHECKLIST.md)

### 系统架构

- **快速概览**：[ARCHITECTURE_SUMMARY.md](./ARCHITECTURE_SUMMARY.md)
- **完整架构**：[REW_ARCHITECTURE.md](./REW_ARCHITECTURE.md)
- **数据流图**：[QUICK_REFERENCE.md](./QUICK_REFERENCE.md#数据流图)

### Hook 系统

- **Hook 是什么**：[CLAUDE_CODE_HOOK_INTEGRATION.md](./CLAUDE_CODE_HOOK_INTEGRATION.md#hook-系统-是什么)
- **技术细节**：[HOOK_ARCHITECTURE.md](./HOOK_ARCHITECTURE.md)
- **集成步骤**：[USER_INSTALLATION_GUIDE.md](./USER_INSTALLATION_GUIDE.md#第二步启用-ai-工具集成)
- **不同工具对比**：[COMPETITOR_HOOK_RESEARCH.md](./COMPETITOR_HOOK_RESEARCH.md)

### 开发指南

- **环境设置**：[DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md#-快速开始)
- **项目结构**：[DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md#-项目结构)
- **添加功能**：[DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md#-常见开发任务)
- **代码风格**：[DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md#-代码风格指南)

### 故障排除

- **安装问题**：[USER_INSTALLATION_GUIDE.md](./USER_INSTALLATION_GUIDE.md#-故障排除)
- **Hook 问题**：[IMPLEMENTATION_CHECKLIST.md](./IMPLEMENTATION_CHECKLIST.md#-故障排除)
- **构建问题**：[DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md#-常见问题)

---

## 📚 快速问答

### Q: 我是新用户，不知道从哪开始？

**A:** 从这个顺序开始：
1. [README.md](./README.md) — 了解 rew 是什么
2. [USER_INSTALLATION_GUIDE.md](./USER_INSTALLATION_GUIDE.md) — 逐步安装
3. [IMPLEMENTATION_CHECKLIST.md](./IMPLEMENTATION_CHECKLIST.md) — 验证安装成功

### Q: 安装过程中出现错误？

**A:** 查看 [USER_INSTALLATION_GUIDE.md#🐛-故障排除](./USER_INSTALLATION_GUIDE.md#🐛-故障排除)

### Q: 我想修改 Hook 系统的代码？

**A:** 按这个顺序：
1. [DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md) — 设置开发环境
2. [HOOK_ARCHITECTURE.md](./HOOK_ARCHITECTURE.md) — 理解 Hook 工作原理
3. [REW_ARCHITECTURE.md](./REW_ARCHITECTURE.md) — 理解整个系统
4. 修改代码并使用 [IMPLEMENTATION_CHECKLIST.md](./IMPLEMENTATION_CHECKLIST.md) 验证

### Q: Hook 系统与其他工具有什么不同？

**A:** 查看 [COMPETITOR_HOOK_RESEARCH.md](./COMPETITOR_HOOK_RESEARCH.md)

### Q: 我想向 rew 贡献代码？

**A:** 按这个顺序：
1. [DEVELOPER_BUILD_GUIDE.md](./DEVELOPER_BUILD_GUIDE.md) — 设置环境和理解代码风格
2. [REW_ARCHITECTURE.md](./REW_ARCHITECTURE.md) — 理解项目结构
3. 提交 PR 前检查 [DEVELOPER_BUILD_GUIDE.md#-贡献指南](./DEVELOPER_BUILD_GUIDE.md#-贡献指南)

---

## 📝 文档维护

这些文档的目标是保持最新和准确。当你发现过时的信息时，请：

1. 打开 GitHub Issue 报告问题
2. 或直接提交 PR 更正

---

## 🎯 本文档地图的用途

这个文档提供了：

1. **快速导航** — 知道自己的角色后，立即找到相关文档
2. **完整索引** — 所有文档的统一入口
3. **学习路径** — 推荐的阅读顺序
4. **关系图** — 理解文档之间的关联
5. **快速查找** — 按主题快速定位内容

---

## 版本信息

- **rew 版本**：0.1.0
- **文档创建日期**：2026-04-09
- **最后更新**：2026-04-09
- **覆盖文档**：11 个主要文档 + 本导航文件

---

**开始阅读：** 从上面 [按角色选择文档](#按角色选择文档) 部分选择最适合你的起点！
