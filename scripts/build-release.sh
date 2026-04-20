#!/bin/bash
set -e

# ============================================================
# rew 发布构建脚本
# 用法：./scripts/build-release.sh
# 完成：tauri build → 签名 CLI 二进制 → 重签 .app → 重打 DMG → 公证 → staple
# ============================================================

IDENTITY="Developer ID Application: xu zhang (TV3TFAJ56J)"
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENTITLEMENTS="$PROJECT_ROOT/src-tauri/entitlements.plist"
APP_PATH="$PROJECT_ROOT/target/release/bundle/macos/rew.app"
CLI_IN_APP="$APP_PATH/Contents/Resources/rew"
DMG_PATH="$PROJECT_ROOT/target/release/bundle/dmg/rew_0.1.0_aarch64.dmg"

echo "🏗️  Step 1: tauri build（签名 .app 和 .dmg）..."
# unset 公证变量，tauri 只签名不公证（后面手动公证）
unset APPLE_API_KEY APPLE_API_KEY_PATH APPLE_API_ISSUER APPLE_API_KEY_ID
cd "$PROJECT_ROOT"
npm run tauri build

echo ""
echo "✍️  Step 2: 对 .app 内的 CLI 二进制重新签名（Hardened Runtime + timestamp）..."
codesign --force \
  --sign "$IDENTITY" \
  --options runtime \
  --timestamp \
  --entitlements "$ENTITLEMENTS" \
  "$CLI_IN_APP"

echo "✍️  Step 3: 对整个 .app 重新签名..."
codesign --force \
  --sign "$IDENTITY" \
  --options runtime \
  --timestamp \
  --entitlements "$ENTITLEMENTS" \
  "$APP_PATH"

echo ""
echo "📦 Step 4: 重新打 DMG..."
rm -f "$DMG_PATH"
hdiutil create \
  -volname "rew" \
  -srcfolder "$APP_PATH" \
  -ov -format UDZO \
  "$DMG_PATH"

echo "✍️  Step 5: 签名 DMG..."
codesign --force \
  --sign "$IDENTITY" \
  --timestamp \
  "$DMG_PATH"

echo ""
echo "🔏 Step 6: 公证 DMG..."
xcrun notarytool submit "$DMG_PATH" \
  --key "$HOME/.private_keys/AuthKey_45PB59369B.p8" \
  --key-id "45PB59369B" \
  --issuer "d99d6102-3f38-4d19-96bb-95930485a359" \
  --wait

echo ""
echo "📎 Step 7: Staple ticket..."
xcrun stapler staple "$APP_PATH"
xcrun stapler staple "$DMG_PATH"

echo ""
echo "🔍 最终验证..."
spctl --assess --type exec --verbose "$APP_PATH"

echo ""
echo "🎉 全部完成！产物路径："
echo "   DMG: $DMG_PATH"
echo "   APP: $APP_PATH"
