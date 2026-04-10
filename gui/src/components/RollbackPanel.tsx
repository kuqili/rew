/**
 * RollbackPanel — "读档"确认面板（内联，不弹 modal）
 *
 * 读档 = 将此节点内所有文件回到改动发生前的状态，就像游戏读档一样。
 * 可以多次读档，操作幂等。
 */
import { useState, useEffect } from "react";
import { previewRollback, rollbackTask, type UndoPreviewInfo } from "../lib/tauri";
import { fileName } from "../lib/format";

interface Props {
  taskId: string;
  isMonitoringWindow: boolean;
  windowLabel?: string;
  onClose: () => void;
  onRolledBack: () => void;
}

export default function RollbackPanel({
  taskId,
  isMonitoringWindow,
  windowLabel,
  onClose,
  onRolledBack,
}: Props) {
  const [preview, setPreview] = useState<UndoPreviewInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [rolling, setRolling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);

  useEffect(() => {
    previewRollback(taskId)
      .then(setPreview)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [taskId]);

  const handleRollback = async () => {
    setRolling(true);
    setError(null);
    try {
      const res = await rollbackTask(taskId);
      const anyDone = res.files_restored > 0 || res.files_deleted > 0;
      const hasFailures = res.failures.length > 0;

      if (!hasFailures && !anyDone) {
        // Task had no recoverable file changes (e.g. all files already in target state)
        setError("此任务没有可恢复的文件变更");
      } else if (!hasFailures) {
        // Full success
        setSuccess(true);
        setTimeout(onRolledBack, 1200);
      } else if (anyDone) {
        // Partial: some succeeded, some failed
        const msg = res.failures.map(([p, e]) => `${fileName(p)}: ${e}`).join("\n");
        setError(`部分文件读档失败：\n${msg}`);
        setTimeout(onRolledBack, 2000);
      } else {
        // All failed
        const msg = res.failures.map(([p, e]) => `${fileName(p)}: ${e}`).join("\n");
        setError(msg || "读档失败，请重试");
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setRolling(false);
    }
  };

  const actionDesc = isMonitoringWindow
    ? `将 ${windowLabel ?? "此时间段"} 内所有文件回到该存档点之前的版本`
    : "将此次 AI 任务涉及的所有文件回到操作执行之前的版本";

  return (
    <div className="border-t border-surface-border bg-surface-secondary">
      <div className="px-5 py-3">
        {/* Header */}
        <div className="flex items-center justify-between mb-2">
          <div className="flex items-center gap-2">
            <span className="text-[12px] font-medium text-ink">↩ 确认读档</span>
          </div>
          <button onClick={onClose} className="text-ink-faint hover:text-ink-secondary text-xs transition-colors">
            取消
          </button>
        </div>

        <p className="text-[11px] text-ink-secondary mb-2">{actionDesc}</p>

        {/* Key notes */}
        <div className="mb-3 space-y-1.5 text-[10px] text-ink-muted leading-relaxed">
          <div className="flex gap-1.5">
            <span className="text-status-green flex-shrink-0">✓</span>
            <span>可随时再次读档，不限次数，操作可撤销</span>
          </div>
          <div className="flex gap-1.5">
            <span className="text-ink-faint flex-shrink-0">ℹ</span>
            <span>读档不会改动不在此存档点内的文件</span>
          </div>
          {preview && preview.files_to_delete.length > 0 && (
            <div className="flex gap-1.5">
              <span className="text-status-red flex-shrink-0">!</span>
              <span>此存档点内有新建的文件，读档后这些文件将被<b>删除</b>（因为它们在存档点记录时尚不存在）</span>
            </div>
          )}
        </div>

        {/* Preview content */}
        {loading ? (
          <div className="py-1 text-[11px] text-ink-muted">分析影响范围...</div>
        ) : error ? (
          <div className="py-2 text-[11px] text-status-red whitespace-pre-wrap bg-status-red-bg rounded px-3 py-2">
            {error}
          </div>
        ) : success ? (
          <div className="py-1 flex items-center gap-2 text-[12px] text-status-green font-medium">
            <span>✓</span>
            <span>读档完成，文件已还原</span>
          </div>
        ) : preview ? (
          <div className="space-y-2 mb-3">
            {preview.files_to_restore.length > 0 && (
              <div>
                <div className="text-[10px] text-ink-muted mb-1">
                  将还原 {preview.files_to_restore.length} 个文件到改动前版本：
                </div>
                <ul className="space-y-0.5">
                  {preview.files_to_restore.slice(0, 6).map((p) => (
                    <li key={p} className="text-[10px] font-mono text-ink-secondary flex items-center gap-1.5">
                      <span className="change-icon bg-status-yellow-bg text-status-yellow" style={{ fontSize: 8, padding: "1px 3px" }}>M</span>
                      {fileName(p)}
                    </li>
                  ))}
                  {preview.files_to_restore.length > 6 && (
                    <li className="text-[10px] text-ink-muted">…另 {preview.files_to_restore.length - 6} 个</li>
                  )}
                </ul>
              </div>
            )}
            {preview.files_to_delete.length > 0 && (
              <div>
                <div className="text-[10px] text-ink-muted mb-1">
                  将删除 {preview.files_to_delete.length} 个新建的文件：
                </div>
                <ul className="space-y-0.5">
                  {preview.files_to_delete.slice(0, 6).map((p) => (
                    <li key={p} className="text-[10px] font-mono text-ink-secondary flex items-center gap-1.5">
                      <span className="change-icon bg-status-red-bg text-status-red" style={{ fontSize: 8, padding: "1px 3px" }}>D</span>
                      {fileName(p)}
                    </li>
                  ))}
                  {preview.files_to_delete.length > 6 && (
                    <li className="text-[10px] text-ink-muted">…另 {preview.files_to_delete.length - 6} 个</li>
                  )}
                </ul>
              </div>
            )}
            {preview.files_to_restore.length === 0 && preview.files_to_delete.length === 0 && (
              <p className="text-[11px] text-ink-muted">没有需要还原的文件（当前状态已是改动前）</p>
            )}
          </div>
        ) : null}

        {/* Confirm button */}
        {!success && !loading && (
          <button
            onClick={handleRollback}
            disabled={rolling || !!error || (preview?.files_to_restore.length === 0 && preview?.files_to_delete.length === 0)}
            className="w-full py-1.5 rounded bg-status-red text-white text-[12px] font-medium hover:opacity-90 transition-opacity disabled:opacity-40"
          >
            {rolling ? "读档中..." : "确认读档"}
          </button>
        )}
      </div>
    </div>
  );
}
