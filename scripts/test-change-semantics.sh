#!/usr/bin/env bash

set -euo pipefail

# 统一回归入口：文件变更语义 / Git 对齐 / 产品链路关键逻辑。
#
# 设计目标：
# 1. 不需要记住分散的 cargo test 命令。
# 2. 默认覆盖“当前最关键”的文件变更语义测试矩阵。
# 3. 便于以后在 CI 或本地回归时直接复用。
#
# 说明：
# - 该脚本会运行 git 金标测试（git_semantics.rs）。
# - git 金标测试会在临时目录里创建临时 git repo，并在测试结束后自动清理。
# - 不会修改当前主仓库的 git 历史，也不会改全局 git config。

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

run() {
  local title="$1"
  shift

  echo
  echo "============================================================"
  echo "${title}"
  echo "============================================================"
  echo "+ $*"
  "$@"
}

echo "开始执行文件变更语义回归测试..."
echo "仓库根目录: ${ROOT_DIR}"

# 1) 核心 DB / baseline / reconcile / dedup 语义回归
run \
  "1/8 核心语义回归（change_tracking + git_semantics）" \
  cargo test -p rew-core --test change_tracking --test git_semantics

# 2) EventProcessor 的 3 秒窗口合并与 dynamic pause 行为
run \
  "2/8 EventProcessor 归并与 dynamic pause" \
  cargo test -p rew-core processor::merge_logic_tests

# 3) PathFilter 默认 ignore / 白名单 / 临时文件规则
run \
  "3/8 PathFilter ignore 规则" \
  cargo test -p rew-core watcher::filter

# 4) 目录恢复 / 目录回档 / 恢复后抑制相关
run \
  "4/8 目录恢复与恢复后行为" \
  cargo test -p rew-core restore::tests

# 5) DB 事务 / 幂等 / bundle 原子性
run \
  "5/8 DB 事务与幂等" \
  cargo test -p rew-core db::tests

# 6) daemon FSEvent 路由：active / grace / monitoring
run \
  "6/8 daemon 路由优先级" \
  cargo test -p rew-tauri daemon::tests

# 7) hook single-writer / stop-finalize / reconcile 全链路
run \
  "7/8 hook 事件全链路" \
  cargo test -p rew-core hook_events::tests

# 8) 各 AI 工具原始 hook payload 归一化
run \
  "8/8 AI 工具 hook payload 归一化" \
  cargo test -p rew-cli commands::hook::tests

echo
echo "文件变更语义回归测试全部通过。"
