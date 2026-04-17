#!/bin/bash
# ─────────────────────────────────────────────────────────────
# rew 一键安装脚本
# 用法：curl -sL https://rew-ai.woa.com/install.sh | bash
# ─────────────────────────────────────────────────────────────
set -e

REPO="kuqili/rew"
INSTALL_DIR="/Applications"
APP_NAME="rew.app"
TMP_DIR=$(mktemp -d)

cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT

echo ""
echo "  ┌─────────────────────────────────┐"
echo "  │  rew — AI 时代的文件安全网       │"
echo "  │  一键安装                        │"
echo "  └─────────────────────────────────┘"
echo ""

# ── 1. 检测架构 ──────────────────────────────────────────────
ARCH=$(uname -m)
if [ "$ARCH" = "arm64" ]; then
  ARCH_LABEL="aarch64"
  echo "  ✓ 检测到 Apple Silicon (arm64)"
elif [ "$ARCH" = "x86_64" ]; then
  ARCH_LABEL="x64"
  echo "  ✓ 检测到 Intel (x86_64)"
else
  echo "  ✗ 不支持的架构: $ARCH"
  exit 1
fi

# ── 2. 获取最新版本 ──────────────────────────────────────────
echo "  → 获取最新版本..."
LATEST=$(curl -sL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
if [ -z "$LATEST" ]; then
  echo "  ✗ 无法获取最新版本，使用 v0.1.0"
  LATEST="v0.1.0"
fi
NUM_VER="${LATEST#v}"
echo "  ✓ 最新版本: $LATEST"

# ── 3. 下载 DMG ─────────────────────────────────────────────
DMG_NAME="rew_${NUM_VER}_${ARCH_LABEL}.dmg"
DMG_URL="https://github.com/$REPO/releases/download/$LATEST/$DMG_NAME"
DMG_PATH="$TMP_DIR/$DMG_NAME"

echo "  → 下载 $DMG_NAME ..."
curl -sL -o "$DMG_PATH" "$DMG_URL"

if [ ! -f "$DMG_PATH" ] || [ ! -s "$DMG_PATH" ]; then
  echo "  ✗ 下载失败"
  exit 1
fi
echo "  ✓ 下载完成"

# ── 4. 挂载 DMG ─────────────────────────────────────────────
echo "  → 挂载 DMG..."
MOUNT_POINT=$(hdiutil attach "$DMG_PATH" -nobrowse -readonly 2>/dev/null | grep "/Volumes" | tail -1 | awk -F'\t' '{print $NF}' | sed 's/^ *//')

if [ -z "$MOUNT_POINT" ] || [ ! -d "$MOUNT_POINT" ]; then
  echo "  ✗ 挂载失败"
  exit 1
fi
echo "  ✓ 已挂载: $MOUNT_POINT"

# ── 5. 关闭旧版本 ───────────────────────────────────────────
if pgrep -f "rew-tauri" >/dev/null 2>&1; then
  echo "  → 关闭正在运行的 rew..."
  pkill -f "rew-tauri" 2>/dev/null || true
  sleep 1
fi

# ── 6. 安装 app ─────────────────────────────────────────────
echo "  → 安装到 $INSTALL_DIR..."
if [ -d "$INSTALL_DIR/$APP_NAME" ]; then
  rm -rf "$INSTALL_DIR/$APP_NAME"
fi
cp -R "$MOUNT_POINT/$APP_NAME" "$INSTALL_DIR/$APP_NAME"
echo "  ✓ 已安装"

# ── 7. 去隔离 + 重签名 ──────────────────────────────────────
echo "  → 处理安全属性..."
xattr -cr "$INSTALL_DIR/$APP_NAME" 2>/dev/null
codesign --force --deep --sign - "$INSTALL_DIR/$APP_NAME" 2>/dev/null
echo "  ✓ 安全属性已处理"

# ── 8. 卸载 DMG ─────────────────────────────────────────────
hdiutil detach "$MOUNT_POINT" -quiet 2>/dev/null || true

# ── 9. 启动 ─────────────────────────────────────────────────
echo ""
echo "  ✅ 安装完成！rew $LATEST"
echo ""
echo "  正在启动 rew..."
open "$INSTALL_DIR/$APP_NAME"
echo ""
echo "  如果没有自动打开，请手动运行："
echo "  open /Applications/rew.app"
echo ""
