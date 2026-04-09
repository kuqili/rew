# rew 发布与分发指南

本文档说明如何把 rew 的 `.dmg` 文件放到官网供用户一键下载，**全程不需要命令行**（除了初次推送代码到 GitHub）。

---

## 整体架构

```
用户点击下载按钮
       ↓
GitHub Releases（托管 .dmg 文件，免费，不限流量）
       ↑
GitHub Actions（自动构建 .dmg，push tag 即触发）
       ↑
git push tag v0.1.0（你只需要做这一步）

官网（GitHub Pages，免费，自动部署）
  website/index.html → 链接到 GitHub Releases
```

---

## 第一步：把代码推送到 GitHub

### 1.1 在 GitHub 创建仓库

1. 打开 [github.com/new](https://github.com/new)
2. Repository name 填 `rew`
3. 选 **Private** 或 **Public**（官网下载需要 Public，或 Private + 付费计划）
4. **不要**勾选初始化 README（本地已有）
5. 点击「Create repository」

### 1.2 关联远程并推送

```bash
cd /Users/kuqili/Desktop/project/rew
git remote add origin https://github.com/kuqili/rew.git
git push -u origin main
git push origin feat/dmg-distribution   # 推送本分支
```

> 把 `kuqili` 替换为你的 GitHub 用户名。

---

## 第二步：开启 GitHub Pages（托管官网）

1. 进入仓库 → **Settings** → **Pages**
2. Source 选 **「GitHub Actions」**（不是 branch）
3. 保存

下次 push `main` 分支时，Actions 会自动把 `website/` 目录部署到：
```
https://kuqili.github.io/rew/
```

> **自定义域名（可选）**：在 Pages 设置里填入你的域名，然后在 DNS 加一条 CNAME 指向 `kuqili.github.io`。

---

## 第三步：发布第一个版本（生成 .dmg）

```bash
# 更新版本号（在 src-tauri/tauri.conf.json 里改 "version"）
# 然后：
git add -A
git commit -m "chore: bump version to 0.1.0"
git tag v0.1.0
git push origin main --tags
```

**这就是全部操作。** GitHub Actions 会自动：

1. 用 macOS runner 构建 Tauri 应用
2. 生成两个 `.dmg` 文件（Apple Silicon + Intel）
3. 创建 GitHub Release，标题「rew v0.1.0」
4. 把 `.dmg` 作为附件上传到 Release

构建大约需要 **10–15 分钟**。完成后，下载链接变为实际可用：
```
https://github.com/kuqili/rew/releases/latest/download/rew_0.1.0_aarch64.dmg
https://github.com/kuqili/rew/releases/latest/download/rew_0.1.0_x64.dmg
```

官网的下载按钮会通过 GitHub API 自动获取最新版本号并更新链接，**无需手动修改 HTML**。

---

## 第四步（可选）：更新官网中的仓库链接

官网目前使用 `kuqili/rew` 作为占位符，如果你的 GitHub 用户名不同，需要全局替换：

```bash
# 在 website/ 目录下批量替换
sed -i '' 's/kuqili\/rew/YOUR_USERNAME\/rew/g' website/index.html website/why.html website/features.html
```

---

## 后续发布流程（极简）

每次有新版本，只需要：

```bash
# 1. 改 src-tauri/tauri.conf.json 里的 version 字段
# 2. 提交并打 tag
git add src-tauri/tauri.conf.json
git commit -m "chore: bump to 0.2.0"
git tag v0.2.0
git push origin main --tags
```

等 15 分钟，GitHub Actions 构建完成，官网下载链接自动指向新版本。

---

## 常见问题

### Q: 构建失败怎么办？

进入 GitHub → Actions → 点击失败的 workflow → 查看日志。
常见原因：
- Rust 编译错误（本地 `cargo build --target aarch64-apple-darwin` 先验证）
- 前端依赖问题（本地 `cd gui && pnpm build` 先验证）

### Q: 用户下载后提示「无法验证开发者」？

这是 macOS Gatekeeper 正常行为（未签名应用）。解决方法：
- 右键 → 打开 → 允许（官网已有说明）
- 或执行：`xattr -cr /Applications/rew.app`

**根本解决**：申请苹果开发者账号（$99/年），完成代码签名和公证（Notarization）后，用户双击即可正常打开。
签名配置参考：`.github/workflows/release.yml` 中的注释。

### Q: .dmg 有多大？

Tauri 应用通常 10–30 MB，GitHub Releases 单文件限制 2 GB，完全没问题。

### Q: 免费吗？

- GitHub Actions：Public 仓库完全免费，Private 仓库每月 2000 分钟免费
- GitHub Pages：Public 仓库完全免费
- GitHub Releases：完全免费，不限带宽
- 总计：**$0/月**

---

## 架构图（完整）

```
开发者
  │
  ├── git push origin main
  │         │
  │         └─→ GitHub Actions: pages.yml
  │                   └─→ 部署 website/ 到 GitHub Pages
  │                           └─→ https://kuqili.github.io/rew/
  │
  └── git push tag v0.1.0
            │
            └─→ GitHub Actions: release.yml
                      ├─→ macOS Runner (arm64): cargo build + tauri bundle
                      ├─→ macOS Runner (x64):  cargo build + tauri bundle
                      └─→ 创建 GitHub Release + 上传 .dmg
                                └─→ https://github.com/kuqili/rew/releases/latest/download/*.dmg
                                          ↑
                          官网下载按钮通过 GitHub API 动态指向此 URL
```
