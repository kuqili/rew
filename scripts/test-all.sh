#!/usr/bin/env bash

set -euo pipefail

# 统一全量回归入口：发布前跑整个 workspace 的测试。
# 说明：
# 1. 与 test-change-semantics.sh 不同，这里追求“全量覆盖”而不是“最快回归”。
# 2. rew-core/tests/backup_restore.rs 已迁入 workspace，可被 cargo test --workspace 直接发现。
# 3. 测试过程中创建的临时目录 / 临时 git repo 都由各测试自行清理，不会污染当前仓库。

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

echo "开始执行全量 workspace 测试..."
echo "仓库根目录: ${ROOT_DIR}"
echo
echo "============================================================"
echo "1/1 cargo test --workspace"
echo "============================================================"
echo "+ cargo test --workspace"
cargo test --workspace
echo
echo "全量 workspace 测试全部通过。"
