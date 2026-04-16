#!/usr/bin/env bash

set -euo pipefail

# 统一全量回归入口：发布前 / CI 打包前必跑，任意失败即阻断。
#
# 执行顺序：
#   1. cargo check          — 编译检查（含 warnings 检测）
#   2. 核心语义测试          — change_tracking / git_semantics / system_integration
#   3. 其他集成测试          — full_journey / backup_restore / performance
#   4. workspace 全量测试    — 覆盖所有 crate 的单元测试

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

PASS=0
FAIL=0
TOTAL_START=$(date +%s)

run_step() {
    local label="$1"
    shift
    echo "============================================================"
    echo "▶ ${label}"
    echo "  $ $*"
    echo "------------------------------------------------------------"
    local start
    start=$(date +%s)
    if "$@"; then
        local elapsed=$(( $(date +%s) - start ))
        echo "  ✓ ${label} 通过 (${elapsed}s)"
        PASS=$((PASS + 1))
    else
        local elapsed=$(( $(date +%s) - start ))
        echo "  ✗ ${label} 失败 (${elapsed}s)"
        FAIL=$((FAIL + 1))
    fi
    echo
}

echo
echo "rew 全量回归测试"
echo "仓库根目录: ${ROOT_DIR}"
echo "开始时间:   $(date '+%Y-%m-%d %H:%M:%S')"
echo

# ── 1. 编译检查 ──────────────────────────────────────────────
run_step "编译检查 (cargo check --workspace)" \
    cargo check --workspace

# ── 2. 核心语义测试（变更追踪 / Git 语义 / 系统集成） ────────
run_step "变更追踪 (change_tracking)" \
    cargo test -p rew-core --test change_tracking

run_step "Git 语义对齐 (git_semantics)" \
    cargo test -p rew-core --test git_semantics

run_step "系统集成 + Oracle 验证 (system_integration)" \
    cargo test -p rew-core --test system_integration

# ── 3. 其他集成测试 ──────────────────────────────────────────
run_step "完整旅程 (full_journey)" \
    cargo test -p rew-core --test full_journey

run_step "备份恢复 (backup_restore)" \
    cargo test -p rew-core --test backup_restore

run_step "性能基准 (performance)" \
    cargo test -p rew-core --test performance

# ── 4. Workspace 全量 ────────────────────────────────────────
run_step "Workspace 全量测试" \
    cargo test --workspace

# ── 汇总 ─────────────────────────────────────────────────────
TOTAL_ELAPSED=$(( $(date +%s) - TOTAL_START ))
echo "============================================================"
echo "汇总: ${PASS} 通过, ${FAIL} 失败 (总耗时 ${TOTAL_ELAPSED}s)"
echo "============================================================"

if [ "${FAIL}" -gt 0 ]; then
    echo "⛔ 存在失败项，阻断发布。"
    exit 1
fi

echo "✅ 全量回归通过，可以发布。"
