import { useState, useEffect } from "react";
import { type TaskInfo, previewUndo, undoTask, type UndoPreviewInfo } from "../lib/tauri";

interface Props {
  taskId: string;
  task: TaskInfo;
  onClose: () => void;
  onUndone: () => void;
}

export default function UndoConfirm({ taskId, task, onClose, onUndone }: Props) {
  const [preview, setPreview] = useState<UndoPreviewInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [undoing, setUndoing] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    previewUndo(taskId)
      .then(setPreview)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [taskId]);

  const handleUndo = async () => {
    setUndoing(true);
    try {
      const res = await undoTask(taskId);
      if (res.failures.length === 0) {
        setResult(`成功：恢复 ${res.files_restored} 个文件，删除 ${res.files_deleted} 个文件`);
        setTimeout(onUndone, 1200);
      } else if (res.files_restored > 0 || res.files_deleted > 0) {
        setResult(`部分成功：恢复 ${res.files_restored} 个，${res.failures.length} 个失败`);
        setTimeout(onUndone, 2000);
      } else {
        // All failed
        const noHash = res.failures.some(([, err]) =>
          err.includes("old_hash") || err.includes("APFS") || err.includes("快照")
        );
        if (noHash) {
          const cancelled = res.failures.some(([, err]) => err.includes("取消"));
          const notFound = res.failures.some(([, err]) => err.includes("未找到"));
          if (cancelled) {
            setError("你取消了授权。请重新点击撤销并输入密码（或使用 Touch ID）。");
          } else if (notFound) {
            setError(
              "此文件在 APFS 快照中未找到。\n\n" +
              "可能的原因：文件在快照创建之后才出现，或快照已被系统清理。\n" +
              "请检查废纸篓是否还有该文件。"
            );
          } else {
            setError(res.failures.map(([p, e]) => `${p}:\n${e}`).join("\n\n"));
          }
        } else {
          setError(res.failures.map(([p, e]) => `${p}:\n${e}`).join("\n\n"));
        }
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setUndoing(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="bg-white rounded-xl border border-surface-border shadow-xl p-6 w-[400px]">
        {/* Header */}
        <h2 className="text-base font-semibold text-ink mb-4 flex items-center gap-2">
          <span>↩</span> 撤销此任务
        </h2>

        {/* Prompt */}
        {task.prompt && (
          <div className="bg-surface-secondary rounded-lg px-4 py-3 mb-4 text-[13px] text-ink border border-surface-border/60">
            "{task.prompt}"
          </div>
        )}

        {/* Preview */}
        {loading ? (
          <div className="py-6 text-center text-ink-muted text-sm">分析中...</div>
        ) : error ? (
          <div className="py-4 text-status-red text-sm whitespace-pre-wrap bg-status-red-bg rounded-lg px-4">
            {error}
          </div>
        ) : result ? (
          <div className="py-4 text-center">
            <div className="text-2xl mb-2">✅</div>
            <div className="text-sm text-status-green">{result}</div>
          </div>
        ) : preview ? (
          <div className="space-y-2 mb-4 text-[13px]">
            <div className="text-ink-secondary mb-3">
              共 {preview.total_changes} 个文件受影响：
            </div>

            {preview.files_to_restore.length > 0 && (
              <div className="flex items-center gap-2">
                <span className="change-icon bg-status-yellow-bg text-status-yellow">M</span>
                <span className="text-ink">
                  恢复 {preview.files_to_restore.length} 个文件到之前的版本
                </span>
              </div>
            )}

            {preview.files_to_delete.length > 0 && (
              <div className="flex items-center gap-2">
                <span className="change-icon bg-status-red-bg text-status-red">D</span>
                <span className="text-ink">
                  删除 {preview.files_to_delete.length} 个新创建的文件
                </span>
              </div>
            )}

            <div className="mt-3 flex items-start gap-2 text-2xs text-ink-muted bg-surface-secondary rounded-lg px-3 py-2 border border-surface-border/60">
              <span>ℹ</span>
              <span>撤销前会自动创建安全快照作为备份</span>
            </div>
          </div>
        ) : null}

        {/* Actions */}
        {!result && (
          <div className="flex gap-3 justify-end pt-2 border-t border-surface-border">
            <button
              onClick={onClose}
              className="px-4 py-2 rounded-md border border-surface-border text-sm text-ink-secondary hover:bg-surface-hover transition-colors"
            >
              取消
            </button>
            {error && (error.includes("请重试") || error.includes("请重新点击")) ? (
              <button
                onClick={() => { setError(null); handleUndo(); }}
                className="px-4 py-2 rounded-md bg-st-blue text-white text-sm font-medium hover:opacity-90 transition-opacity"
              >
                重试撤销
              </button>
            ) : (
              <button
                onClick={handleUndo}
                disabled={loading || undoing || !!error}
                className="px-4 py-2 rounded-md bg-status-red text-white text-sm font-medium hover:opacity-90 transition-opacity disabled:opacity-40"
              >
                {undoing ? "撤销中..." : "确认撤销"}
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
