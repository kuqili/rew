// rew — Main application entry point
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ==================== Types ====================
interface SnapshotInfo {
  id: string;
  timestamp: string;
  trigger: string;
  files_added: number;
  files_modified: number;
  files_deleted: number;
  pinned: boolean;
  is_anomaly: boolean;
  metadata_json: string | null;
}

interface StatusInfo {
  running: boolean;
  paused: boolean;
  watch_dirs: string[];
  total_snapshots: number;
  anomaly_count: number;
  has_warning: boolean;
}

interface RestorePreviewInfo {
  snapshot_id: string;
  files_to_restore: number;
  files_to_overwrite: number;
  files_to_remove: number;
  estimated_size_bytes: number;
}

interface ConfigInfo {
  watch_dirs: string[];
  ignore_patterns: string[];
}

// ==================== State ====================
let currentSnapshots: SnapshotInfo[] = [];
let selectedSnapshotId: string | null = null;
let currentPage: "setup" | "timeline" = "timeline";

// ==================== App Init ====================
async function init() {
  const app = document.getElementById("app")!;

  try {
    const isFirstRun = await invoke<boolean>("check_first_run");
    if (isFirstRun) {
      currentPage = "setup";
      renderSetup(app);
    } else {
      currentPage = "timeline";
      renderTimeline(app);
      await loadSnapshots();
    }
  } catch (e) {
    console.error("Init error:", e);
    currentPage = "timeline";
    renderTimeline(app);
    await loadSnapshots();
  }

  // Listen for anomaly events
  await listen("anomaly-detected", (event: any) => {
    const data = event.payload;
    showToast(`⚠️ 异常检测: ${data.description}`, "error");
    if (data.snapshot_id && currentPage === "timeline") {
      selectedSnapshotId = data.snapshot_id;
      loadSnapshots();
    }
  });
}

// ==================== Setup Wizard ====================
async function renderSetup(container: HTMLElement) {
  let defaultDirs: { path: string; label: string; checked: boolean }[] = [];

  try {
    const config = await invoke<ConfigInfo>("get_config");
    defaultDirs = config.watch_dirs.map((dir) => {
      const name = dir.split("/").pop() || dir;
      const labelMap: Record<string, string> = {
        Desktop: "桌面 (Desktop)",
        Documents: "文档 (Documents)",
        Downloads: "下载 (Downloads)",
      };
      return {
        path: dir,
        label: labelMap[name] || name,
        checked: true,
      };
    });
  } catch {
    // Fallback
    defaultDirs = [
      { path: "~/Desktop", label: "桌面 (Desktop)", checked: true },
      { path: "~/Documents", label: "文档 (Documents)", checked: true },
      { path: "~/Downloads", label: "下载 (Downloads)", checked: true },
    ];
  }

  container.innerHTML = `
    <div class="setup-page">
      <div class="setup-card">
        <h1>👋 欢迎使用 rew</h1>
        <p class="subtitle">AI 时代的文件安全网 — 自动保护你的重要文件</p>

        <h2>选择要保护的目录</h2>
        <ul class="dir-list" id="dir-list">
          ${defaultDirs
            .map(
              (d, i) => `
            <li onclick="this.querySelector('input').click()">
              <input type="checkbox" id="dir-${i}" data-path="${d.path}" ${d.checked ? "checked" : ""} onclick="event.stopPropagation()">
              <span class="dir-label">${d.label}</span>
            </li>
          `
            )
            .join("")}
        </ul>

        <div class="add-dir-row">
          <input type="text" id="custom-dir-input" placeholder="添加自定义目录路径..." />
          <button class="btn btn-sm" id="add-dir-btn">添加</button>
        </div>

        <button class="btn btn-primary btn-block" id="start-btn">
          🛡️ 开始保护
        </button>
      </div>
    </div>
  `;

  document.getElementById("add-dir-btn")!.addEventListener("click", () => {
    const input = document.getElementById(
      "custom-dir-input"
    ) as HTMLInputElement;
    const path = input.value.trim();
    if (!path) return;

    const list = document.getElementById("dir-list")!;
    const idx = list.children.length;
    const li = document.createElement("li");
    li.setAttribute("onclick", "this.querySelector('input').click()");
    li.innerHTML = `
      <input type="checkbox" id="dir-${idx}" data-path="${path}" checked onclick="event.stopPropagation()">
      <span class="dir-label">${path}</span>
    `;
    list.appendChild(li);
    input.value = "";
  });

  document
    .getElementById("start-btn")!
    .addEventListener("click", async () => {
      const checkboxes = document.querySelectorAll(
        '#dir-list input[type="checkbox"]:checked'
      );
      const dirs: string[] = [];
      checkboxes.forEach((cb: any) => dirs.push(cb.dataset.path));

      if (dirs.length === 0) {
        showToast("请至少选择一个目录", "error");
        return;
      }

      try {
        await invoke("complete_setup", { watchDirs: dirs });
        showToast("保护已开启！", "success");

        // Transition to timeline
        currentPage = "timeline";
        renderTimeline(container);
        await loadSnapshots();
      } catch (e) {
        showToast(`设置失败: ${e}`, "error");
      }
    });
}

// ==================== Timeline View ====================
function renderTimeline(container: HTMLElement) {
  container.innerHTML = `
    <div class="timeline-page">
      <div class="sidebar">
        <div class="sidebar-header">
          <h1>rew</h1>
          <div class="status-badge normal" id="status-badge">
            <span class="status-dot"></span>
            <span id="status-text">运行中</span>
          </div>
        </div>
        <div class="snapshot-list" id="snapshot-list">
          <div class="empty-state">
            <div class="icon">📸</div>
            <div class="title">暂无快照</div>
            <div class="desc">文件变更时将自动创建快照</div>
          </div>
        </div>
      </div>
      <div class="detail-panel" id="detail-panel">
        <div class="detail-empty">
          选择一个快照查看详情
        </div>
      </div>
    </div>
  `;

  updateStatus();
}

async function updateStatus() {
  try {
    const status = await invoke<StatusInfo>("get_status");
    const badge = document.getElementById("status-badge");
    const text = document.getElementById("status-text");
    if (!badge || !text) return;

    badge.className = "status-badge";
    if (status.paused) {
      badge.classList.add("paused");
      text.textContent = "已暂停";
    } else if (status.has_warning) {
      badge.classList.add("warning");
      text.textContent = "有告警";
    } else {
      badge.classList.add("normal");
      text.textContent = "运行中";
    }
  } catch (e) {
    console.error("Status error:", e);
  }
}

async function loadSnapshots() {
  try {
    currentSnapshots = await invoke<SnapshotInfo[]>("list_snapshots");
    renderSnapshotList();
    if (selectedSnapshotId) {
      renderDetail(selectedSnapshotId);
    }
  } catch (e) {
    console.error("Load snapshots error:", e);
  }
}

function renderSnapshotList() {
  const container = document.getElementById("snapshot-list");
  if (!container) return;

  if (currentSnapshots.length === 0) {
    container.innerHTML = `
      <div class="empty-state">
        <div class="icon">📸</div>
        <div class="title">暂无快照</div>
        <div class="desc">文件变更时将自动创建快照</div>
      </div>
    `;
    return;
  }

  // Group by date
  const groups = new Map<string, SnapshotInfo[]>();
  for (const snap of currentSnapshots) {
    const date = new Date(snap.timestamp);
    const key = formatDate(date);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(snap);
  }

  let html = "";
  for (const [date, snaps] of groups) {
    html += `<div class="date-group">`;
    html += `<div class="date-label">${date}</div>`;
    for (const snap of snaps) {
      const isActive = snap.id === selectedSnapshotId;
      const isAnomaly = snap.is_anomaly;
      const time = formatTime(new Date(snap.timestamp));
      const triggerClass =
        snap.trigger === "anomaly"
          ? "trigger-anomaly"
          : snap.trigger === "manual"
            ? "trigger-manual"
            : "trigger-auto";
      const triggerLabel =
        snap.trigger === "anomaly"
          ? "⚠️ 异常"
          : snap.trigger === "manual"
            ? "手动"
            : "自动";

      html += `
        <div class="snapshot-item${isActive ? " active" : ""}${isAnomaly ? " anomaly" : ""}"
             data-id="${snap.id}" onclick="window.__selectSnapshot('${snap.id}')">
          <span class="snapshot-time">${time}</span>
          <div class="snapshot-meta">
            <span class="snapshot-trigger ${triggerClass}">${triggerLabel}</span>
            <div class="snapshot-stats">
              ${snap.files_added > 0 ? `<span class="stat-add">+${snap.files_added}</span> ` : ""}
              ${snap.files_modified > 0 ? `<span class="stat-mod">~${snap.files_modified}</span> ` : ""}
              ${snap.files_deleted > 0 ? `<span class="stat-del">-${snap.files_deleted}</span>` : ""}
              ${snap.files_added === 0 && snap.files_modified === 0 && snap.files_deleted === 0 ? '<span style="color:var(--text-muted)">无变更</span>' : ""}
            </div>
          </div>
          ${snap.pinned ? '<span class="pin-icon">📌</span>' : ""}
        </div>
      `;
    }
    html += `</div>`;
  }

  container.innerHTML = html;
}

function renderDetail(snapshotId: string) {
  selectedSnapshotId = snapshotId;
  const panel = document.getElementById("detail-panel");
  if (!panel) return;

  const snap = currentSnapshots.find((s) => s.id === snapshotId);
  if (!snap) {
    panel.innerHTML = `<div class="detail-empty">快照未找到</div>`;
    return;
  }

  const date = new Date(snap.timestamp);
  const fullTime = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
  const triggerLabel =
    snap.trigger === "anomaly"
      ? "⚠️ 异常触发"
      : snap.trigger === "manual"
        ? "手动创建"
        : "自动快照";

  panel.innerHTML = `
    <div class="detail-header">
      <div>
        <div class="detail-title">${fullTime}</div>
        <span class="snapshot-trigger ${snap.trigger === "anomaly" ? "trigger-anomaly" : snap.trigger === "manual" ? "trigger-manual" : "trigger-auto"}" style="margin-top:4px;display:inline-flex">
          ${triggerLabel}
        </span>
      </div>
      <div class="detail-actions">
        <button class="btn btn-sm" onclick="window.__togglePin('${snap.id}')">
          ${snap.pinned ? "📌 取消固定" : "📌 固定"}
        </button>
        <button class="btn btn-sm btn-primary" onclick="window.__showRestore('${snap.id}')">
          ↩️ 恢复
        </button>
      </div>
    </div>
    <div class="detail-body">
      <div class="info-grid">
        <div class="info-card">
          <div class="value" style="color:var(--green)">${snap.files_added}</div>
          <div class="label">新增文件</div>
        </div>
        <div class="info-card">
          <div class="value" style="color:var(--yellow)">${snap.files_modified}</div>
          <div class="label">修改文件</div>
        </div>
        <div class="info-card">
          <div class="value" style="color:var(--red)">${snap.files_deleted}</div>
          <div class="label">删除文件</div>
        </div>
      </div>

      <div class="detail-section">
        <h3>快照信息</h3>
        <div style="font-size:0.85rem;color:var(--text-secondary);">
          <p>ID: <span style="font-family:var(--font-mono);color:var(--text-muted)">${snap.id.slice(0, 8)}...</span></p>
          <p>触发类型: ${triggerLabel}</p>
          <p>固定状态: ${snap.pinned ? "已固定（永久保留）" : "未固定"}</p>
          ${snap.is_anomaly ? '<p style="color:var(--red);margin-top:0.5rem">⚠️ 此快照由异常检测规则触发，请检查文件变更是否符合预期</p>' : ""}
        </div>
      </div>
    </div>
  `;

  // Re-render list to update active state
  renderSnapshotList();
}

// ==================== Restore Modal ====================
async function showRestoreModal(snapshotId: string) {
  const snap = currentSnapshots.find((s) => s.id === snapshotId);
  if (!snap) return;

  try {
    const preview = await invoke<RestorePreviewInfo>("get_restore_preview", {
      snapshotId,
    });

    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    overlay.innerHTML = `
      <div class="modal">
        <h2>↩️ 确认恢复</h2>
        <div class="preview-stats">
          <div class="preview-stat">
            <div class="num" style="color:var(--green)">${preview.files_to_restore}</div>
            <div class="desc">将恢复的文件</div>
          </div>
          <div class="preview-stat">
            <div class="num" style="color:var(--yellow)">${preview.files_to_overwrite}</div>
            <div class="desc">将覆盖的文件</div>
          </div>
          <div class="preview-stat">
            <div class="num" style="color:var(--red)">${preview.files_to_remove}</div>
            <div class="desc">将移除的文件</div>
          </div>
        </div>
        <div class="warning-text">
          ⚠️ 恢复操作将修改文件系统，恢复前会自动创建安全快照作为保险。
        </div>
        <div id="restore-progress" style="display:none">
          <div class="progress-bar">
            <div class="progress-fill" id="progress-fill" style="width:0%"></div>
          </div>
          <p style="font-size:0.85rem;color:var(--text-muted);text-align:center" id="restore-status">正在恢复...</p>
        </div>
        <div class="modal-actions" id="restore-actions">
          <button class="btn" id="cancel-restore">取消</button>
          <button class="btn btn-primary" id="confirm-restore">确认恢复</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    overlay.addEventListener("click", (e) => {
      if (e.target === overlay)
        overlay.remove();
    });
    overlay.querySelector("#cancel-restore")!.addEventListener("click", () => {
      overlay.remove();
    });
    overlay
      .querySelector("#confirm-restore")!
      .addEventListener("click", async () => {
        // Show progress
        overlay.querySelector<HTMLElement>("#restore-actions")!.style.display =
          "none";
        overlay.querySelector<HTMLElement>("#restore-progress")!.style.display =
          "block";

        // Animate progress
        const fill = overlay.querySelector<HTMLElement>("#progress-fill")!;
        const status = overlay.querySelector<HTMLElement>("#restore-status")!;
        fill.style.width = "30%";
        status.textContent = "正在创建安全快照...";

        try {
          const result = await invoke<string>("restore_snapshot", {
            snapshotId,
          });
          fill.style.width = "100%";
          status.textContent = "恢复完成！";

          setTimeout(() => {
            overlay.remove();
            showToast("✅ " + result, "success");
            loadSnapshots();
          }, 1000);
        } catch (e) {
          fill.style.width = "100%";
          fill.style.background = "var(--red)";
          status.textContent = `恢复失败: ${e}`;
          status.style.color = "var(--red)";

          setTimeout(() => {
            overlay.remove();
            showToast(`恢复失败: ${e}`, "error");
          }, 2000);
        }
      });
  } catch (e) {
    showToast(`加载预览失败: ${e}`, "error");
  }
}

// ==================== Actions ====================
async function togglePin(snapshotId: string) {
  try {
    const newPinned = await invoke<boolean>("toggle_pin", { snapshotId });
    showToast(newPinned ? "📌 已固定快照" : "已取消固定", "success");
    await loadSnapshots();
  } catch (e) {
    showToast(`操作失败: ${e}`, "error");
  }
}

// ==================== Utilities ====================
function formatDate(date: Date): string {
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);

  if (sameDay(date, today)) return "今天";
  if (sameDay(date, yesterday)) return "昨天";
  return `${date.getMonth() + 1}月${date.getDate()}日`;
}

function sameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

function formatTime(date: Date): string {
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function pad(n: number): string {
  return n < 10 ? `0${n}` : `${n}`;
}

function showToast(message: string, type: "success" | "error" = "success") {
  const existing = document.querySelector(".toast");
  if (existing) existing.remove();

  const toast = document.createElement("div");
  toast.className = `toast ${type}`;
  toast.textContent = message;
  document.body.appendChild(toast);

  setTimeout(() => toast.remove(), 4000);
}

// ==================== Global Handlers ====================
(window as any).__selectSnapshot = (id: string) => {
  renderDetail(id);
};

(window as any).__togglePin = (id: string) => {
  togglePin(id);
};

(window as any).__showRestore = (id: string) => {
  showRestoreModal(id);
};

// ==================== Start ====================
document.addEventListener("DOMContentLoaded", init);
