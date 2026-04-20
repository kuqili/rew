#!/bin/bash
set -euo pipefail

# ============================================================
# rew 构建脚本
#
# 用法：
#   ./scripts/build-release.sh           # 本地安装模式（默认）
#   ./scripts/build-release.sh --release # 公证发版模式（出 DMG）
#
# 本地模式：测试 → 编译 → 打包 → 安装到 /Applications → 启动
# 发版模式：编译 → 打包 → 签名 → 公证 → staple → 输出 DMG
# ============================================================

MODE="local"
if [[ "${1:-}" == "--release" ]]; then
  MODE="release"
fi

IDENTITY="Developer ID Application: xu zhang (TV3TFAJ56J)"
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DST="/Applications/rew.app"
ENTITLEMENTS="$PROJECT_ROOT/src-tauri/entitlements.plist"

cd "$PROJECT_ROOT"

# ── Step 1: 全量测试（仅本地模式）────────────────────────────
if [[ "$MODE" == "local" ]]; then
  echo "== 1) 全量测试 =="
  ./scripts/test-all.sh
fi

# ── Step 2: 编译 CLI ──────────────────────────────────────────
echo "== 2) 构建最新 CLI =="
cargo build --release -p rew-cli
cp "$PROJECT_ROOT/target/release/rew" "$PROJECT_ROOT/src-tauri/rew"

# ── Step 3: 编译前端 ──────────────────────────────────────────
echo "== 3) 构建最新前端 =="
cd "$PROJECT_ROOT/gui" && pnpm build
cd "$PROJECT_ROOT"

# ── Step 4: 打包 app（不公证，后面手动处理）──────────────────
echo "== 4) 打包 Tauri app =="
unset APPLE_API_KEY APPLE_API_KEY_PATH APPLE_API_ISSUER APPLE_API_KEY_ID
CI=true npm run tauri build -- --bundles dmg,app

APP_PATH="$PROJECT_ROOT/target/release/bundle/macos/rew.app"
DMG_PATH=$(ls "$PROJECT_ROOT/target/release/bundle/dmg/rew_"*.dmg 2>/dev/null | head -1 || echo "")

# ── Step 5: 签名 CLI 二进制（发版模式）───────────────────────
if [[ "$MODE" == "release" ]]; then
  echo "== 5) 签名 CLI 二进制 =="
  codesign --force \
    --sign "$IDENTITY" \
    --options runtime \
    --timestamp \
    --entitlements "$ENTITLEMENTS" \
    "$APP_PATH/Contents/Resources/rew"

  echo "== 5b) 重签 .app =="
  codesign --force \
    --sign "$IDENTITY" \
    --options runtime \
    --timestamp \
    --entitlements "$ENTITLEMENTS" \
    "$APP_PATH"

  echo "== 5c) 重打 DMG =="
  rm -f "$DMG_PATH"
  hdiutil create \
    -volname "rew" \
    -srcfolder "$APP_PATH" \
    -ov -format UDZO \
    "$PROJECT_ROOT/target/release/bundle/dmg/rew_release.dmg"
  DMG_PATH="$PROJECT_ROOT/target/release/bundle/dmg/rew_release.dmg"

  codesign --force --sign "$IDENTITY" --timestamp "$DMG_PATH"

  echo "== 5d) 公证 =="
  xcrun notarytool submit "$DMG_PATH" \
    --key "$HOME/.private_keys/AuthKey_45PB59369B.p8" \
    --key-id "45PB59369B" \
    --issuer "d99d6102-3f38-4d19-96bb-95930485a359" \
    --wait

  echo "== 5e) Staple =="
  xcrun stapler staple "$APP_PATH"
  xcrun stapler staple "$DMG_PATH"

  echo "== 5f) 验证 =="
  spctl --assess --type exec --verbose "$APP_PATH"

  echo ""
  echo "🎉 发版完成！"
  echo "   DMG: $DMG_PATH"
  exit 0
fi

# ── 本地模式：安装到 /Applications ───────────────────────────
SRC_BIN="$APP_PATH/Contents/MacOS/rew-tauri"
DST_BIN="$APP_DST/Contents/MacOS/rew-tauri"

echo "== 5) 关闭旧进程 =="
osascript -e 'quit app "rew"' 2>/dev/null || true
pkill -9 -f "rew-tauri" 2>/dev/null || true
sleep 1

echo "== 6) 覆盖安装到 /Applications =="
if [ -d "$APP_DST" ]; then
  rm -rf "$APP_DST"
fi
ditto "$APP_PATH" "$APP_DST"

echo "== 7) 二进制一致性校验 =="
SRC_SHA=$(shasum -a 256 "$SRC_BIN" | awk '{print $1}')
DST_SHA=$(shasum -a 256 "$DST_BIN" | awk '{print $1}')
echo "SRC: $SRC_SHA"
echo "DST: $DST_SHA"
[ "$SRC_SHA" = "$DST_SHA" ] || { echo "❌ 安装失败：目标不是新包"; exit 1; }

echo "== 8) 启动 =="
open -n "$APP_DST"

echo ""
echo "✅ 本地安装完成，rew 已启动！"
