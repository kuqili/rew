import { useState, useEffect } from "react";
import { useScanProgress } from "../hooks/useScanProgress";
import {
  analyzeDirectories, getStorageInfo, addWatchDir, removeWatchDir,
  type FullAnalysis, type StorageInfo, type DirScanStatus, type DirAnalysis,
} from "../lib/tauri";

function fmt(b: number): string {
  if (b === 0) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(b) / Math.log(1024));
  return (b / Math.pow(1024, i)).toFixed(i > 1 ? 1 : 0) + " " + u[i];
}

interface Props { onClose: () => void; }

export default function SettingsPanel({ onClose }: Props) {
  const [tab, setTab] = useState<"dirs" | "about">("dirs");
  const [analysis, setAnalysis] = useState<FullAnalysis | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [expandedDir, setExpandedDir] = useState<string | null>(null);
  const scanProgress = useScanProgress();

  useEffect(() => {
    setAnalyzing(true);
    analyzeDirectories().then(setAnalysis).catch(console.error).finally(() => setAnalyzing(false));
    getStorageInfo().then(setStorage).catch(() => {});
    const timer = setInterval(() => {
      getStorageInfo().then(setStorage).catch(() => {});
    }, 3000);
    return () => clearInterval(timer);
  }, []);

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <div className="flex-shrink-0 bg-surface-secondary border-b border-surface-border px-6 py-4 flex items-center justify-between">
        <h2 className="text-[15px] font-semibold text-ink">设置</h2>
        <button onClick={onClose} className="text-ink-muted hover:text-ink text-lg">×</button>
      </div>
      <div className="flex-shrink-0 border-b border-surface-border px-6 flex">
        {([{ k: "dirs" as const, l: "目录管理" }, { k: "about" as const, l: "关于" }]).map((t) => (
          <button key={t.k} onClick={() => setTab(t.k)}
            className={`px-4 py-2.5 text-[13px] border-b-2 transition-colors ${tab === t.k ? "border-st-blue text-st-blue font-medium" : "border-transparent text-ink-secondary hover:text-ink"}`}>
            {t.l}
          </button>
        ))}
      </div>
      <div className="flex-1 overflow-y-auto px-6 py-5">
        {tab === "dirs" ? (
          <div className="space-y-6 max-w-[680px]">
            {/* Global overview */}
            <div className="bg-surface-secondary rounded-lg px-4 py-3 border border-surface-border/60">
              {analyzing ? (
                <div className="flex items-center gap-2 text-[13px] text-ink-secondary">
                  <span className="inline-block animate-spin">◐</span> 正在分析保护目录...
                </div>
              ) : analysis ? (
                <div className="space-y-3">
                  {/* Row 1: scope */}
                  <div>
                    <div className="text-2xs text-ink-muted mb-0.5">保护范围</div>
                    <div className="flex items-baseline gap-2">
                      <span className="text-[17px] font-semibold text-ink">{analysis.total_files.toLocaleString()}</span>
                      <span className="text-[13px] text-ink-muted">个文件</span>
                      <span className="text-[13px] text-ink-secondary">· {fmt(analysis.total_bytes)}</span>
                    </div>
                    <div className="flex flex-wrap gap-x-4 gap-y-0.5 text-2xs text-ink-muted mt-1">
                      {analysis.dirs.map((d) => (
                        <span key={d.path}>{d.path.split("/").pop()}: {fmt(d.total_bytes)}</span>
                      ))}
                    </div>
                  </div>

                  {/* Row 2: backup progress */}
                  {storage && (
                    <div>
                      <div className="text-2xs text-ink-muted mb-0.5">备份进度</div>
                      {storage.object_count >= analysis.total_files ? (
                        <div className="flex items-center gap-2">
                          <span className="text-[15px] font-semibold text-status-green">✓ 全部已备份</span>
                          <span className="text-[13px] text-ink-muted">{storage.object_count.toLocaleString()} 个文件</span>
                        </div>
                      ) : (
                        <>
                          <div className="flex items-baseline gap-2">
                            <span className="text-[15px] font-semibold text-st-blue">{storage.object_count.toLocaleString()}</span>
                            <span className="text-[13px] text-ink-muted">/ {analysis.total_files.toLocaleString()} 个文件已备份</span>
                          </div>
                          <div className="w-full h-1.5 bg-surface-border rounded-full overflow-hidden mt-1.5">
                            <div className="h-full bg-st-blue rounded-full transition-all duration-300"
                              style={{ width: `${Math.min((storage.object_count / analysis.total_files) * 100, 100)}%` }} />
                          </div>
                        </>
                      )}
                    </div>
                  )}

                  {/* Row 3: storage note */}
                  {storage && (
                    <div className="text-2xs text-ink-muted leading-relaxed px-1">
                      使用 APFS clonefile 技术备份，与原文件共享磁盘空间，不产生额外占用。
                      原文件被修改或删除后，备份才会独立占用空间。
                    </div>
                  )}
                </div>
              ) : null}
            </div>

            {/* Directory list */}
            <div>
              <div className="flex items-center justify-between mb-2">
                <h3 className="text-[13px] font-semibold text-ink">保护目录</h3>
                <button onClick={async () => {
                  try {
                    const { open } = await import("@tauri-apps/plugin-dialog");
                    const s = await open({ directory: true, multiple: false });
                    if (s) await addWatchDir(s as string);
                  } catch {}
                }} className="px-3 py-1 rounded-md bg-st-blue text-white text-[13px] font-medium hover:opacity-90">+ 添加</button>
              </div>
              <div className="space-y-2">
                {(scanProgress?.dirs || []).map((dir) => (
                  <DirCard
                    key={dir.path}
                    dir={dir}
                    da={analysis?.dirs.find((d) => d.path === dir.path) || null}
                    expanded={expandedDir === dir.path}
                    onToggle={() => setExpandedDir(expandedDir === dir.path ? null : dir.path)}
                    onRemove={async () => {
                      if (confirm(`停止保护「${dir.name}」？\n已备份的文件不会被删除。`))
                        await removeWatchDir(dir.path);
                    }}
                  />
                ))}
              </div>
            </div>
          </div>
        ) : (
          <div className="space-y-4 max-w-[640px]">
            <h3 className="text-[13px] font-semibold text-ink">rew — AI 时代的文件安全网</h3>
            <p className="text-2xs text-ink-secondary leading-relaxed">
              实时监控文件，AI 工具操作时自动备份。误删或改错，一键撤销。
            </p>
            <div className="text-2xs text-ink-muted space-y-1">
              <div>版本: 0.1.0</div>
              <div>存储: APFS clonefile (CoW)</div>
              {storage && <div>备份: {storage.object_count.toLocaleString()} 对象 · {fmt(storage.apparent_bytes)}</div>}
            </div>
            <div className="mt-4 pt-3 border-t border-surface-border">
              <h4 className="text-2xs font-semibold text-ink mb-2">默认不备份的文件类型</h4>
              <div className="text-2xs text-ink-muted space-y-1 leading-relaxed">
                <div>• <b>应用程序</b> — .app 包内文件（Xcode、VS Code 等）</div>
                <div>• <b>安装包</b> — .dmg, .pkg, .iso, .msi, .exe</div>
                <div>• <b>开发产物</b> — node_modules, .git, target, __pycache__</div>
                <div>• <b>系统临时文件</b> — .DS_Store, Thumbs.db, .swp</div>
              </div>
              <p className="text-2xs text-ink-muted mt-2">
                这些文件可以重新下载或自动生成，不属于用户的敏感数据。
                其余所有文件（文档、代码、图片、表格等）都会被自动备份保护。
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// ─── Directory Card ───

function DirCard({ dir, da, expanded, onToggle, onRemove }: {
  dir: DirScanStatus; da: DirAnalysis | null; expanded: boolean;
  onToggle: () => void; onRemove: () => void;
}) {
  return (
    <div className="border border-surface-border rounded-lg overflow-hidden">
      <button onClick={onToggle} className="w-full text-left px-4 py-3 hover:bg-surface-hover/50 transition-colors">
        <div className="flex items-center justify-between mb-0.5">
          <div className="flex items-center gap-2">
            <span className="text-[13px] font-medium text-ink">{dir.name}</span>
            {dir.status === "complete" && <span className="badge bg-status-green-bg text-status-green">已保护</span>}
            {dir.status === "scanning" && <span className="badge bg-status-yellow-bg text-status-yellow">扫描中 {dir.percent.toFixed(0)}%</span>}
            {dir.status === "pending" && <span className="badge bg-surface-secondary text-ink-muted">等待</span>}
          </div>
          <span className="text-2xs text-ink-muted">{expanded ? "▾" : "▸"}</span>
        </div>
        <div className="text-2xs text-ink-muted">{dir.path}</div>
        {da && (
          <div className="text-2xs text-ink-secondary mt-1">
            {da.total_files.toLocaleString()} 个文件 · {fmt(da.total_bytes)}
          </div>
        )}
        {dir.status === "scanning" && (
          <div className="mt-2 w-full bg-surface-border rounded-full h-1 overflow-hidden">
            <div className="h-full bg-st-blue rounded-full transition-all duration-300" style={{ width: `${Math.min(dir.percent, 100)}%` }} />
          </div>
        )}
      </button>

      {expanded && da && (
        <div className="border-t border-surface-border bg-surface-secondary/50 px-4 py-3">
          <div className="text-2xs font-medium text-ink-secondary mb-2">文件类型分布</div>
          {da.categories.length > 0 ? (
            <div className="space-y-1.5">
              {da.categories.map((c) => {
                const pct = da.total_bytes > 0 ? (c.total_bytes / da.total_bytes) * 100 : 0;
                return (
                  <div key={c.category} className="flex items-center gap-2 text-2xs">
                    <div className="w-[70px] truncate text-ink-secondary">{c.category}</div>
                    <div className="flex-1 h-1.5 bg-surface-border rounded-full overflow-hidden">
                      <div className="h-full rounded-full bg-st-blue" style={{ width: `${Math.max(pct, 1)}%` }} />
                    </div>
                    <div className="w-[55px] text-right tabular-nums text-ink-muted">{fmt(c.total_bytes)}</div>
                    <div className="w-[45px] text-right tabular-nums text-ink-muted">{c.file_count} 个</div>
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="text-2xs text-ink-muted">无分类数据</div>
          )}
          <div className="mt-3 pt-2 border-t border-surface-border/60">
            <button onClick={(e) => { e.stopPropagation(); onRemove(); }} disabled={dir.status === "scanning"}
              className="text-2xs text-ink-faint hover:text-status-red transition-colors disabled:opacity-30">
              移除此目录
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
