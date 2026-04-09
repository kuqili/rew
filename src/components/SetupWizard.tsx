import { useState, useEffect } from "react";
import { completeSetup, getConfig } from "../lib/tauri";

interface Props {
  onComplete: () => void;
}

export default function SetupWizard({ onComplete }: Props) {
  const [dirs, setDirs] = useState<{ path: string; label: string; checked: boolean }[]>([]);
  const [customDir, setCustomDir] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    getConfig()
      .then((config) => {
        const labelMap: Record<string, string> = {
          Desktop: "桌面 (Desktop)",
          Documents: "文档 (Documents)",
          Downloads: "下载 (Downloads)",
        };
        setDirs(
          config.watch_dirs.map((d) => {
            const name = d.split("/").pop() || d;
            return { path: d, label: labelMap[name] || name, checked: true };
          })
        );
      })
      .catch(() => {
        setDirs([
          { path: "~/Desktop", label: "桌面 (Desktop)", checked: true },
          { path: "~/Documents", label: "文档 (Documents)", checked: true },
          { path: "~/Downloads", label: "下载 (Downloads)", checked: true },
        ]);
      });
  }, []);

  const handleStart = async () => {
    const selected = dirs.filter((d) => d.checked).map((d) => d.path);
    if (selected.length === 0) return;
    setLoading(true);
    try {
      await completeSetup(selected);
      onComplete();
    } catch (e) {
      console.error("Setup failed:", e);
    } finally {
      setLoading(false);
    }
  };

  const addDir = () => {
    if (!customDir.trim()) return;
    setDirs([...dirs, { path: customDir.trim(), label: customDir.trim(), checked: true }]);
    setCustomDir("");
  };

  return (
    <div className="flex items-center justify-center h-screen bg-surface-secondary">
      <div className="bg-white rounded-xl shadow-panel border border-surface-border p-8 w-[400px]">
        <div className="text-center mb-6">
          <div className="text-3xl mb-2">🛡️</div>
          <h1 className="text-lg font-semibold text-ink mb-1">欢迎使用 rew</h1>
          <p className="text-2xs text-ink-muted">AI 时代的安全带 — 自动保护你的文件</p>
        </div>

        <div className="mb-4">
          <div className="text-2xs font-medium text-ink-secondary mb-2 uppercase tracking-wider">
            选择保护目录
          </div>
          <div className="space-y-1">
            {dirs.map((d, i) => (
              <label
                key={i}
                className="flex items-center gap-3 px-3 py-2 rounded-md hover:bg-surface-secondary cursor-pointer transition-colors"
              >
                <input
                  type="checkbox"
                  checked={d.checked}
                  onChange={() => {
                    const next = [...dirs];
                    next[i].checked = !next[i].checked;
                    setDirs(next);
                  }}
                  className="w-3.5 h-3.5 rounded accent-st-blue"
                />
                <span className="text-[13px] text-ink flex-1">{d.label}</span>
                <span className="text-2xs text-ink-faint font-mono">{d.path.split("/").pop()}</span>
              </label>
            ))}
          </div>
        </div>

        <div className="flex gap-2 mb-5">
          <input
            type="text"
            value={customDir}
            onChange={(e) => setCustomDir(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addDir()}
            placeholder="添加自定义目录..."
            className="flex-1 px-3 py-1.5 rounded-md border border-surface-border text-[13px] text-ink placeholder:text-ink-faint focus:outline-none focus:border-st-blue focus:ring-1 focus:ring-st-blue/30"
          />
          <button
            onClick={addDir}
            className="px-3 py-1.5 rounded-md border border-surface-border text-[13px] text-ink-secondary hover:bg-surface-hover transition-colors"
          >
            添加
          </button>
        </div>

        <button
          onClick={handleStart}
          disabled={loading || dirs.filter((d) => d.checked).length === 0}
          className="w-full py-2.5 rounded-md bg-st-blue text-white text-[13px] font-medium hover:bg-st-blue-hover transition-colors disabled:opacity-40"
        >
          {loading ? "启动中..." : "开始保护"}
        </button>
      </div>
    </div>
  );
}
