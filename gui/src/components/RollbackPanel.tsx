/**
 * RollbackPanel — "读档"确认面板（内联，不弹 modal）
 */
import { useState, useEffect } from "react";
import { RotateCcw, CheckCircle, AlertTriangle, Info } from "lucide-react";
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
        setError("此任务没有可恢复的文件变更");
      } else if (!hasFailures) {
        setSuccess(true);
        setTimeout(onRolledBack, 1200);
      } else if (anyDone) {
        const msg = res.failures.map(([p, e]) => `${fileName(p)}: ${e}`).join("\n");
        setError(`部分文件读档失败：\n${msg}`);
        setTimeout(onRolledBack, 2000);
      } else {
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
    ? `将 ${windowLabel ?? "此时间段"} 内所有文件与目录回到该存档点之前的版本`
    : "将此次 AI 任务涉及的所有文件回到操作执行之前的版本";

  return (
    <div className="border-b border-surface-border bg-surface-primary/80">
      <div className="px-5 py-4">
        {/* Header */}
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-2">
            <RotateCcw className="w-4 h-4 text-safe-blue" />
            <span className="text-[13px] font-semibold text-ink">确认读档</span>
          </div>
          <button onClick={onClose} className="text-ink-muted hover:text-ink text-[12px] transition-colors">
            取消
          </button>
        </div>

        <p className="text-[12px] text-ink-secondary mb-3">{actionDesc}</p>

        {/* Notes */}
        <div className="mb-3 space-y-2">
          <div className="flex gap-2 text-[11px] text-ink-muted items-start">
            <CheckCircle className="w-3.5 h-3.5 text-success-green flex-shrink-0 mt-0.5" />
            <span>可随时再次读档，不限次数，操作可撤销</span>
          </div>
          <div className="flex gap-2 text-[11px] text-ink-muted items-start">
            <Info className="w-3.5 h-3.5 text-ink-faint flex-shrink-0 mt-0.5" />
            <span>读档不会改动不在此存档点内的文件</span>
          </div>
          {preview && preview.files_to_delete.length > 0 && (
            <div className="flex gap-2 text-[11px] text-danger-red items-start">
              <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0 mt-0.5" />
              <span>此存档点内有新建的文件，读档后这些文件将被<b>删除</b></span>
            </div>
          )}
        </div>

        {/* Preview / Status */}
        {loading ? (
          <div className="py-2 text-[12px] text-ink-muted flex items-center gap-2">
            <span className="animate-spin">◐</span> 分析影响范围...
          </div>
        ) : error ? (
          <div className="py-2 text-[12px] text-danger-red whitespace-pre-wrap bg-danger-red-light rounded-xl px-4 py-3">
            {error}
          </div>
        ) : success ? (
          <div className="py-2 flex items-center gap-2 text-[13px] text-success-green font-medium">
            <CheckCircle className="w-4 h-4" />
            读档完成，文件已还原
          </div>
        ) : preview ? (
          <div className="space-y-2 mb-3">
            {preview.files_to_restore.length > 0 && (
              <div>
                <div className="text-[11px] text-ink-muted mb-1">
                  将还原 {preview.files_to_restore.length} 个文件:
                </div>
                <ul className="space-y-0.5">
                  {preview.files_to_restore.slice(0, 5).map((p) => (
                    <li key={p} className="text-[11px] font-mono text-ink-secondary flex items-center gap-1.5">
                      <span className="change-icon bg-warn-yellow-light text-warn-yellow" style={{ fontSize: 8, padding: "1px 3px", width: 14, height: 14 }}>M</span>
                      {fileName(p)}
                    </li>
                  ))}
                  {preview.files_to_restore.length > 5 && (
                    <li className="text-[10px] text-ink-muted">…另 {preview.files_to_restore.length - 5} 个</li>
                  )}
                </ul>
              </div>
            )}
            {preview.files_to_delete.length > 0 && (
              <div>
                <div className="text-[11px] text-ink-muted mb-1">
                  将删除 {preview.files_to_delete.length} 个新建的文件:
                </div>
                <ul className="space-y-0.5">
                  {preview.files_to_delete.slice(0, 5).map((p) => (
                    <li key={p} className="text-[11px] font-mono text-ink-secondary flex items-center gap-1.5">
                      <span className="change-icon bg-danger-red-light text-danger-red" style={{ fontSize: 8, padding: "1px 3px", width: 14, height: 14 }}>D</span>
                      {fileName(p)}
                    </li>
                  ))}
                  {preview.files_to_delete.length > 5 && (
                    <li className="text-[10px] text-ink-muted">…另 {preview.files_to_delete.length - 5} 个</li>
                  )}
                </ul>
              </div>
            )}
            {preview.files_to_restore.length === 0 && preview.files_to_delete.length === 0 && (
              <p className="text-[12px] text-ink-muted">没有需要还原的文件</p>
            )}
          </div>
        ) : null}

        {/* Confirm button */}
        {!success && !loading && (
          <button
            onClick={handleRollback}
            disabled={rolling || !!error || (preview?.files_to_restore.length === 0 && preview?.files_to_delete.length === 0)}
            className="w-full py-2 rounded-xl bg-danger-red text-white text-[13px] font-medium hover:opacity-90 transition-opacity disabled:opacity-40"
          >
            {rolling ? "读档中..." : "确认读档"}
          </button>
        )}
      </div>
    </div>
  );
}
