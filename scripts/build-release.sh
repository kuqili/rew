#!/bin/bash
set -euo pipefail

# ============================================================
# rew 构建脚本
#
# 用法：
#   ./scripts/build-release.sh           # 本地安装模式（默认）
#   ./scripts/build-release.sh --release # 公证发版模式（出 DMG）
# ============================================================

MODE="local"
if [[ "${1:-}" == "--release" ]]; then
  MODE="release"
fi

IDENTITY="Developer ID Application: xu zhang (TV3TFAJ56J)"
ROOT="/Users/kuqili/Desktop/project/rew"
APP_DST="/Applications/rew.app"
ENTITLEMENTS="$ROOT/src-tauri/entitlements.plist"
SRC_APP="$ROOT/src-tauri/target/release/bundle/macos/rew.app"
SRC_BIN="$SRC_APP/Contents/MacOS/rew-tauri"
DST_BIN="$APP_DST/Contents/MacOS/rew-tauri"

cd "$ROOT"

# ── Step 1: 全量测试（仅本地模式）────────────────────────────
if [[ "$MODE" == "local" ]]; then
  echo "== 1) 全量测试（失败即中断） =="
  ./scripts/test-all.sh
fi

# ── Step 2: 编译 CLI ──────────────────────────────────────────
echo "== 2) 构建最新 CLI =="
cargo build --release -p rew-cli
cp "$ROOT/target/release/rew" "$ROOT/src-tauri/rew"

# ── Step 3: 编译前端 ──────────────────────────────────────────
echo "== 3) 构建最新前端 =="
cd "$ROOT/gui" && pnpm build
cd "$ROOT"

# ── Step 4: 打包 app ─────────────────────────────────────────
echo "== 4) 打包最新 App =="
unset APPLE_API_KEY APPLE_API_KEY_PATH APPLE_API_ISSUER APPLE_API_KEY_ID
# tauri.conf.json 配置了 pubkey，本地构建需要同时提供私钥
# 私钥由 `cargo tauri signer generate` 生成，存放在 ~/.tauri/rew.key
export TAURI_SIGNING_PRIVATE_KEY=$(cat "$HOME/.tauri/rew.key")
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
# Cargo 使用内容 hash 做 fingerprint，tauri.conf.json 变化不一定触发重编。
# 直接删掉旧产物，强制 Cargo 重新编译并链接，确保最新配置嵌入二进制。
rm -f "$ROOT/src-tauri/target/release/rew-tauri"
rm -f "$ROOT/src-tauri/target/release/librew_tauri_lib.rlib"
CI=true cargo tauri build --bundles app

# ── 本地模式：安装到 /Applications ───────────────────────────
if [[ "$MODE" == "local" ]]; then
  echo "== 5) 关闭旧进程 =="
  osascript -e 'quit app "rew"' 2>/dev/null || true
  pkill -9 -f "rew-tauri" 2>/dev/null || true
  sleep 1

  echo "== 6) 覆盖安装到 /Applications =="
  if [ -d "$APP_DST" ]; then
    rm -rf "$APP_DST"
  fi
  ditto "$SRC_APP" "$APP_DST"

  # 强制把本次编译的 CLI 写进 App bundle 和 hook 调用路径。
  # cargo tauri build 可能复用缓存导致 bundle 里的 CLI 是旧版本；
  # 而 install_cli_binary() 用 bundle 内容覆盖 ~/.rew/bin/rew，
  # 会把 Step 2 新编译的 CLI 再覆盖回旧的。这里统一刷新两处。
  echo "== 6b) 刷新 App bundle 和 ~/.rew/bin/ 里的 CLI =="
  NEW_CLI="$ROOT/target/release/rew"
  cp "$NEW_CLI" "$APP_DST/Contents/Resources/rew"
  chmod 755 "$APP_DST/Contents/Resources/rew"
  mkdir -p "$HOME/.rew/bin"
  cp "$NEW_CLI" "$HOME/.rew/bin/rew"
  chmod 755 "$HOME/.rew/bin/rew"
  echo "   App bundle CLI: $(ls -la "$APP_DST/Contents/Resources/rew" | awk '{print $6,$7,$8}')"
  echo "   ~/.rew/bin/rew: $(ls -la "$HOME/.rew/bin/rew" | awk '{print $6,$7,$8}')"

  echo "== 7) 二进制一致性校验（必须一致） =="
  SRC_SHA=$(shasum -a 256 "$SRC_BIN" | awk '{print $1}')
  DST_SHA=$(shasum -a 256 "$DST_BIN" | awk '{print $1}')
  echo "SRC: $SRC_SHA"
  echo "DST: $DST_SHA"
  [ "$SRC_SHA" = "$DST_SHA" ] || { echo "❌ 安装失败：目标不是新包"; exit 1; }

  echo "== 8) 启动刚安装的 app =="
  open -n "$APP_DST"

  echo ""
  echo "✅ 本地安装完成，rew 已启动！"
  exit 0
fi

# ── 发版模式：签名 + 公证 + staple ──────────────────────────
DMG_DIR="$ROOT/src-tauri/target/release/bundle/dmg"
DMG_PATH=$(ls "$DMG_DIR/rew_"*.dmg 2>/dev/null | head -1 || echo "")

echo "== 5) 签名 CLI 二进制 =="
codesign --force \
  --sign "$IDENTITY" \
  --options runtime \
  --timestamp \
  --entitlements "$ENTITLEMENTS" \
  "$SRC_APP/Contents/Resources/rew"

echo "== 5b) 重签 .app =="
codesign --force \
  --sign "$IDENTITY" \
  --options runtime \
  --timestamp \
  --entitlements "$ENTITLEMENTS" \
  "$SRC_APP"

echo "== 5c) 重打 DMG =="
rm -f "$DMG_PATH"
hdiutil create \
  -volname "rew" \
  -srcfolder "$SRC_APP" \
  -ov -format UDZO \
  "$DMG_DIR/rew_release.dmg"
DMG_PATH="$DMG_DIR/rew_release.dmg"
codesign --force --sign "$IDENTITY" --timestamp "$DMG_PATH"

echo "== 5d) 公证 =="
xcrun notarytool submit "$DMG_PATH" \
  --key "$HOME/.private_keys/AuthKey_45PB59369B.p8" \
  --key-id "45PB59369B" \
  --issuer "d99d6102-3f38-4d19-96bb-95930485a359" \
  --wait

echo "== 5e) Staple =="
xcrun stapler staple "$SRC_APP"
xcrun stapler staple "$DMG_PATH"

echo "== 5f) 验证 =="
spctl --assess --type exec --verbose "$SRC_APP"

echo ""
echo "🎉 发版完成！"
echo "   DMG: $DMG_PATH"
