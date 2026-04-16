# rew 系统主流程图

## 一、全局视角：两条管线，一个归宿

```mermaid
graph LR
    subgraph 触发源
        AI["🤖 AI 工具操作文件"]
        USER["👤 用户手动编辑"]
    end

    subgraph 感知层
        HOOK["Hook 管线<br/>(AI 主动上报)"]
        FSEVENT["FSEvent 管线<br/>(OS 被动监听)"]
    end

    subgraph 存储层
        TASK["Task<br/>(谁在什么时候做了什么)"]
        CHANGE["Change<br/>(哪个文件从A变成了B)"]
    end

    subgraph 收尾层
        RECONCILE["Reconcile 对账<br/>(去除噪音, 还原真相)"]
        RESTORE["Restore 读档<br/>(撤销变更, 恢复文件)"]
    end

    AI -->|"触发两条管线"| HOOK
    AI -->|"操作产生 FSEvent"| FSEVENT
    USER -->|"操作产生 FSEvent"| FSEVENT

    HOOK -->|"创建/归属 Task<br/>写入 Change"| TASK
    FSEVENT -->|"归属 Task<br/>写入 Change"| TASK
    TASK --- CHANGE

    CHANGE --> RECONCILE
    CHANGE --> RESTORE
```

---

## 二、Hook 管线：AI 任务的完整生命周期

> AI 工具通过 4 种 hook 事件主动上报，rew 据此管理 Task 的生死和 Change 的写入。

```mermaid
flowchart TD
    subgraph "阶段 1：任务创建"
        P["prompt hook<br/>AI 开始一轮对话"]
        P --> CREATE["创建 Task (Active)<br/>绑定 session"]
        P --> OLD{"同 session<br/>有旧 Task?"}
        OLD -->|是| CLOSE_OLD["旧 Task → Completed"]
    end

    subgraph "阶段 2：变更采集"
        PRE["pre-tool hook<br/>AI 准备操作文件"]
        PRE --> SNAPSHOT["快照文件当前状态<br/>(pre-tool hash)"]

        POST["post-tool hook<br/>AI 完成操作文件"]
        POST --> DETECT["检测文件变化:<br/>存在? 内容? 类型?"]
        DETECT --> BASELINE["resolve_baseline<br/>确定'操作前'状态"]
        BASELINE --> WRITE_C["写入 Change<br/>(old_hash → new_hash)"]
    end

    subgraph "阶段 3：任务结束"
        STOP["stop hook<br/>AI 结束本轮对话"]
        STOP --> COMPLETE["Task → Completed"]
        COMPLETE --> ENQUEUE["入队 Finalization"]
        ENQUEUE --> RECON["reconcile_task<br/>对账 + 清理"]
        RECON --> ROLLUP["汇总统计<br/>(changes_count 等)"]
    end

    CREATE --> PRE
    SNAPSHOT --> POST
    WRITE_C --> STOP
```

---

## 三、FSEvent 管线：文件监听的完整处理链

> 所有文件变更（无论 AI 还是手动）都会产生 OS 事件，rew 监听后走独立管线。

```mermaid
flowchart TD
    subgraph "感知"
        FS["文件系统事件<br/>(Create / Modify / Delete / Rename)"]
        FS --> FILTER["过滤噪音<br/>(.git/ node_modules/ 等)"]
        FILTER --> AGG["3 秒窗口聚合<br/>(Created+Deleted=抵消<br/>Created+Modified=Created)"]
    end

    subgraph "归属"
        AGG --> ROUTE{"当前有活跃<br/>AI Task?"}
        ROUTE -->|"是"| AI_TASK["归属到 AI Task<br/>(fsevent_active)"]
        ROUTE -->|"15s 内刚结束"| GRACE["归属到刚结束的 AI Task<br/>(fsevent_grace)"]
        ROUTE -->|"否"| MON["归属到监控窗<br/>(monitoring)"]
    end

    subgraph "写入"
        AI_TASK --> BL["resolve_baseline<br/>确定'操作前'状态"]
        GRACE --> BL
        MON --> MW["管理监控窗 Task<br/>(自动创建/复用/密封)"]
        MW --> BL
        BL --> DEDUP{"去重检查<br/>(Hook 已记录?)"}
        DEDUP -->|"是"| SKIP["跳过"]
        DEDUP -->|"否"| UPSERT["写入 Change<br/>(upsert_change)"]
    end
```

---

## 四、双管线交汇：同一文件的竞争写入

> AI 用 Bash 操作文件时，Hook 和 FSEvent 会同时为同一文件写入 Change，系统通过优先级机制保证一致性。

```mermaid
flowchart TD
    BASH["AI 执行 Bash 命令<br/>(echo > file / rm file)"]

    BASH --> H["Hook 管线写入<br/>attribution = bash_predicted"]
    BASH --> D["FSEvent 管线写入<br/>attribution = fsevent_active"]

    H --> SLOT["changes 表<br/>同一 (task_id, file_path)<br/>只有一个槽位"]
    D --> SLOT

    SLOT --> RULE["upsert_change 规则"]

    RULE --> R1["规则 1: Hook 源先到?<br/>→ 拒绝 daemon 覆盖"]
    RULE --> R2["规则 2: daemon 先到?<br/>→ Hook 源可覆盖,<br/>且 old_hash 以 Hook 为准"]
    RULE --> R3["规则 3: old_hash 不变量<br/>→ 非 Hook 源不得将<br/>old_hash 从 None 提升为 Some"]

    R1 --> SAFE["✅ 最终结果一致"]
    R2 --> SAFE
    R3 --> SAFE
```

---

## 五、Task 的生与死

```mermaid
stateDiagram-v2
    [*] --> Active : prompt hook / 新建监控窗
    Active --> Completed : stop hook / 监控窗过期 / 新 prompt 顶替
    Completed --> Finalizing : finalization worker 消费
    Finalizing --> Completed : reconcile + rollup 完成

    Completed --> RolledBack : 用户读档 (全部成功)
    Completed --> PartialRolledBack : 用户读档 (部分成功)

    note right of Active : 变更持续写入
    note right of Completed : 等待对账
    note right of Finalizing : 清理噪音 + 汇总统计
```

---

## 六、Change 的一生

```mermaid
flowchart LR
    subgraph 写入
        W1["Hook 写入<br/>(hook / bash_predicted)"]
        W2["FSEvent 写入<br/>(fsevent_active / grace / monitoring)"]
    end

    subgraph 存储
        SLOT["changes 表槽位<br/>每个 (task_id, path) 唯一"]
    end

    subgraph 对账
        R1["old == current → 删除<br/>(net-zero)"]
        R2["None == None → 删除<br/>(临时文件)"]
        R3["类型/行数纠正"]
        R4["Deleted+Created → Renamed<br/>(rename 配对)"]
    end

    subgraph 最终
        KEEP["✅ 保留: 真实变更"]
        DEL["🗑 清除: 噪音/临时"]
    end

    W1 --> SLOT
    W2 --> SLOT
    SLOT --> R1
    SLOT --> R2
    SLOT --> R3
    SLOT --> R4
    R1 --> DEL
    R2 --> DEL
    R3 --> KEEP
    R4 --> KEEP
```

---

## 七、Baseline：回答"操作前文件是什么样"

> 无论 Hook 还是 FSEvent，写入 Change 前都要确定 old_hash。Baseline 是共享的判定逻辑。

```mermaid
flowchart TD
    Q["文件 X 在本任务开始前<br/>是否存在? 内容是什么?"]

    Q --> L1{"本任务已有记录?"}
    L1 -->|"Created → 不存在"| ANS_NO["existed = false"]
    L1 -->|"Deleted(None,None) → 临时"| ANS_NO
    L1 -->|"其他 → 存在"| ANS_YES["existed = true<br/>hash = old_hash"]

    L1 -->|"无记录"| L2{"有 pre-tool 快照?"}
    L2 -->|"有"| ANS_YES

    L2 -->|"无"| L3{"前一个任务有记录?"}
    L3 -->|"前任务删了它"| FI_DEL["查 file_index<br/>(可能已被 restore 恢复)"]
    L3 -->|"前任务改/建过它"| FI_CHK["查 file_index tombstone<br/>(可能已被 restore 撤销)"]
    FI_CHK -->|"tombstone"| ANS_NO
    FI_CHK -->|"live / 无记录"| ANS_YES

    L3 -->|"无"| L4{"启动扫描见过?"}
    L4 -->|"是"| ANS_YES
    L4 -->|"否"| ANS_NEVER["从未见过<br/>existed = false"]
```

---

## 八、Restore 与 Baseline 的交互

> 读档操作（undo_task / undo_file / undo_directory）会改变文件系统状态，
> 但**不会删除**历史 change 记录。Baseline 通过 file_index 交叉校验感知 restore。

三种 restore 操作都通过 `apply_file_index_updates_after_restore` 更新 file_index：
- 文件被删除 → `mark_file_index_deleted`（tombstone: `exists_now=0`）
- 文件被恢复 → `upsert_live_file_index_entry`（live: `exists_now=1`）

Baseline Layer 3 在查到前序 task 的历史记录后，对称地检查 file_index：

| 历史记录类型 | file_index 状态 | Baseline 结果 |
|---|---|---|
| Deleted | live（restore 恢复了文件） | existed=true |
| Deleted | 无 live 行 | existed=false |
| Created/Modified | tombstone（restore 删除了文件） | existed=false |
| Created/Modified | live / 无记录 | existed=true |

---

## 九、测试体系

> 发布前通过 `scripts/test-all.sh` 执行全量回归，任意失败阻断打包。

```mermaid
flowchart TD
    subgraph 核心语义
        CT["change_tracking<br/>55 tests<br/>单文件: baseline / upsert / reconcile"]
        GS["git_semantics<br/>17 tests<br/>rew vs git diff 交叉验证"]
        SI["system_integration<br/>62 tests<br/>端到端: Hook→DB→reconcile→oracle"]
    end

    subgraph 集成测试
        FJ["full_journey<br/>检测 / 快照 / 恢复"]
        BR["backup_restore<br/>备份引擎"]
        PF["performance<br/>性能基准"]
    end

    subgraph 全量
        WS["cargo test --workspace<br/>所有 crate 单元测试"]
    end

    CT --> WS
    GS --> WS
    SI --> WS
    FJ --> WS
    BR --> WS
    PF --> WS
```

**Oracle 验证**：`system_integration` 中的关键测试附带独立 oracle —— 在 task 开始前拍摄磁盘快照（path→SHA-256），task 结束后用 baseline→disk 差异计算 ground truth，与 rew 的 reconcile 输出比对。这与 `git_semantics` 用 `git diff --name-status` 做独立裁判的思路一致，避免测试自己验证自己。

---

## 十、一句话总结

```
AI 操作 → Hook 主动上报(管理 Task 生命周期 + 写入 Change)
         ↘
          ↘ 同时
         ↗
OS 监听 → FSEvent 被动捕获(归属到 Task + 写入 Change)
                    ↓
              resolve_baseline (确定操作前状态, 含 restore 交叉校验)
                    ↓
              upsert_change (优先级保护, 去重)
                    ↓
              reconcile_task (对账: 去噪音, 配重命名)
                    ↓
              最终结果: 每个文件一条干净的 Change 记录
```
