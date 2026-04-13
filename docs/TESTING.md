# Testing Guide

本文档只整理当前与“文件变更语义 / Git 对齐 / 产品链路”直接相关的测试入口。

## 推荐入口

日常改动文件变更逻辑后，优先跑这一条：

```bash
./scripts/test-change-semantics.sh
```

这个脚本会顺序执行 4 组测试：

1. `change_tracking.rs`
2. `git_semantics.rs`
3. `processor::merge_logic_tests`
4. `watcher::filter`
5. `daemon::tests`

虽然底层是多条 `cargo test`，但建议统一走脚本入口，这样不需要手动记命令。

## 每组测试的职责

### 1. `crates/rew-core/tests/change_tracking.rs`

职责：

- 校验 `resolve_baseline`
- 校验 `upsert_change`
- 校验 `reconcile_task`
- 校验 dedup / attribution / 边界行为

特点：

- 不依赖真实 git repo
- 运行很快
- 适合作为核心回归层

### 2. `crates/rew-core/tests/git_semantics.rs`

职责：

- 在临时 git repo 中执行真实文件操作
- 对拍 Git 与 rew 的最终语义

当前对拍的 Git 输出：

- `git diff --name-status -M`
- `git status --porcelain`
- `git diff --numstat -M`

判定原则：

- `A` 对齐 `Created`
- `D` 对齐 `Deleted`
- `M` 对齐 `Modified`
- `Rxxx` 对齐 `Renamed`

说明：

- 测试会创建临时 git repo
- 测试结束后临时目录会自动清理
- 不会改主仓库历史，也不会改全局 git config

### 3. `processor::merge_logic_tests`

职责：

- 校验 3 秒窗口内事件归并规则
- 校验 `Created -> Deleted => None` 等表驱动场景
- 校验 package pause / `.git/HEAD` global pause

### 4. `watcher::filter`

职责：

- 校验默认 ignore
- 校验白名单目录
- 校验锁文件 / 临时文件 / 开发产物过滤

### 5. `daemon::tests`

职责：

- 校验 FSEvent 路由优先级
- `active AI task`
- `grace task`
- `monitoring`

## 单独运行命令

如果你只想跑其中一部分，可以直接用这些命令：

```bash
# 核心语义回归 + Git 对拍
cargo test -p rew-core --test change_tracking --test git_semantics

# EventProcessor 归并与 dynamic pause
cargo test -p rew-core processor::merge_logic_tests

# PathFilter 规则
cargo test -p rew-core watcher::filter

# daemon 路由
cargo test -p rew-tauri daemon::tests
```

## 更大范围的回归

如果你要跑 `rew-core` 全量：

```bash
cargo test -p rew-core
```

如果你要跑整个 workspace：

```bash
cargo test --workspace
```

但要注意，这两个命令会包含和“文件变更语义”无关的其它测试，因此日常回归不建议直接拿它们替代 `test-change-semantics.sh`。

## 什么时候必须跑

以下变更后，建议至少跑一次 `./scripts/test-change-semantics.sh`：

- 修改 `baseline.rs`
- 修改 `reconcile.rs`
- 修改 `db.rs` 里和 `changes/tasks` 相关的逻辑
- 修改 `processor.rs`
- 修改 `daemon.rs`
- 修改 hook/daemon attribution / dedup / grace 逻辑
- 修改 rename pairing / Git 语义相关逻辑

## 当前覆盖边界

当前这套回归已经能证明：

- 第一批核心文件变更场景与 Git 对拍一致
- 核心 DB 语义、dedup、reconcile、rename 配对可回归
- processor / filter / daemon 的关键链路有测试保护

但还不能代表“所有极端场景都完全证明完毕”。

后续建议继续补：

- case-only rename
- 同目录多个 rename 同时发生
- 多候选 rename 最优匹配专门用例
- 更完整的 hook/daemon 黑盒竞争
- 随机序列 / fuzz 风格对拍
