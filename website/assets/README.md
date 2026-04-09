# website/assets 说明

此目录存放官网所需的静态资源。

## 当前文件

| 文件 | 说明 |
|------|------|
| `screenshot-placeholder.svg` | Hero 区 App 截图占位图，替换为真实产品截图 |
| `demo-placeholder.svg` | Demo 区视频封面占位图，替换为实际演示视频封面 |

## 待替换的资源

1. **App 截图**（Hero 区 MacBook Mockup 内部）
   - 建议尺寸：1800×1080px，PNG 格式
   - 替换后修改 `index.html` 中 `.mockup-screen` 内容，改为 `<img src="./assets/screenshot.png" ...>`

2. **演示视频**（Demo 区）
   - 上传视频到 YouTube / 腾讯视频 / 自有 CDN
   - 替换 `index.html` 中 `demo-frame` 的点击行为，改为嵌入视频或跳转链接

3. **下载文件**（Download 区）
   - 将打包好的 `rew_0.1.0_aarch64.dmg` 放入此目录
   - 或修改 `index.html` 中下载链接指向 CDN 地址

4. **App 图标**（可选，用于 favicon）
   - 从 `src-tauri/icons/` 复制 `128x128.png`，命名为 `favicon.png`
   - 在 `index.html` `<head>` 中添加 `<link rel="icon" href="./assets/favicon.png">`
