import { useState, useEffect } from "react";
import { completeSetup, setMonitoringWindow, getScanProgress, type ScanProgressInfo } from "../lib/tauri";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Props {
  onComplete: () => void;
}

type Step = "welcome" | "dirs" | "scanning" | "interval" | "done";

const STEPS: { key: Step; label: string }[] = [
  { key: "welcome", label: "欢迎" },
  { key: "dirs", label: "目录" },
  { key: "scanning", label: "初始化" },
  { key: "interval", label: "存档频率" },
];

const WINDOW_OPTIONS = [
  { secs: 300, label: "每 5 分钟", tag: "高频", desc: "适合频繁使用 AI 工具改动文件的场景。" },
  { secs: 600, label: "每 10 分钟", tag: "推荐", desc: "存档数量适中，磁盘占用少，适合大多数人。" },
  { secs: 1800, label: "每 30 分钟", tag: null, desc: "偶尔使用 AI 工具、不需要密集保护的场景。" },
  { secs: 3600, label: "每 1 小时", tag: null, desc: "磁盘占用极少，但相邻存档之间的变更较多。" },
];

// ─── Brand Icon (SVG) ─────────────────────────────────────────────
function BrandIcon() {
  return (
    <svg viewBox="0 0 100 100" className="w-[17px] h-[17px]" style={{ fill: "white" }}>
      <path d="M25 25 H75 V55 H50 L35 70 V55 H25 Z" />
      <path d="M48 40 L40 48 L32 40" stroke="#3a3a3c" strokeWidth="8" fill="none" />
    </svg>
  );
}

// ─── Sidebar ──────────────────────────────────────────────────────
function Sidebar({ current }: { current: Step }) {
  const stepIdx = STEPS.findIndex((s) => s.key === current);
  // "done" = all steps completed
  const isDone = current === "done";

  return (
    <aside className="w-[192px] flex-shrink-0 flex flex-col" style={{ background: "rgba(0,0,0,0.015)", borderRight: "0.5px solid rgba(0,0,0,0.06)", padding: "32px 28px 24px" }}>
      {/* Brand */}
      <div className="mb-10">
        <div className="w-8 h-8 rounded-lg flex items-center justify-center mb-[7px]" style={{ background: "#3a3a3c" }}>
          <BrandIcon />
        </div>
        <div className="text-[10px] font-semibold uppercase" style={{ color: "#aeaeb2", letterSpacing: "0.1em" }}>rew</div>
      </div>

      {/* Step nav */}
      <nav className="relative">
        {/* Vertical line — exactly between first and last dot (height = gap * 3 = 72px) */}
        <div className="absolute w-px" style={{ left: 3, top: 3, height: 72, background: "rgba(0,0,0,0.05)" }} />
        <div className="flex flex-col gap-6 relative">
          {STEPS.map((s, i) => {
            const isActive = !isDone && s.key === current;
            const isPast = isDone || i < stepIdx;
            return (
              <div key={s.key} className="flex items-center gap-3" style={{ fontSize: 12, color: isActive ? "#1d1d1f" : "#c7c7cc", fontWeight: isActive ? 500 : 400 }}>
                <div
                  className="w-[7px] h-[7px] rounded-full flex-shrink-0 relative z-[1]"
                  style={{
                    background: isActive || isPast ? "#007aff" : "white",
                    border: isActive || isPast ? "1.5px solid #007aff" : "1.5px solid #d1d1d6",
                  }}
                />
                {s.label}
              </div>
            );
          })}
        </div>
      </nav>
    </aside>
  );
}

// ─── Action Bar ───────────────────────────────────────────────────
function Actions({ children }: { children: React.ReactNode }) {
  return (
    <div className="mt-auto pt-5 flex items-center justify-end gap-4">
      {children}
    </div>
  );
}

function BtnPrimary({ children, onClick, disabled }: { children: React.ReactNode; onClick: () => void; disabled?: boolean }) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className="text-[13px] font-medium text-white rounded-[5px] cursor-default disabled:opacity-40"
      style={{ background: "#007aff", padding: "4px 18px", border: "none", boxShadow: "inset 0 0.5px 0 rgba(255,255,255,0.2)" }}
    >
      {children}
    </button>
  );
}

function BtnSkip({ children, onClick }: { children: React.ReactNode; onClick: () => void }) {
  return (
    <button onClick={onClick} className="text-[12px] font-medium cursor-default border-none bg-transparent" style={{ color: "#aeaeb2" }}>
      {children}
    </button>
  );
}

// ─── Step 1: Welcome ─────────────────────────────────────────────
function StepWelcome({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  return (
    <>
      <div className="flex-1 max-w-[400px]">
        <h1 className="text-[17px] font-semibold mb-1.5" style={{ color: "#1d1d1f", letterSpacing: "-0.01em" }}>
          像倒带一样回到任意一刻
        </h1>
        <p className="text-[13px] leading-[1.5] mb-6" style={{ color: "#6e6e73" }}>
          rew 在后台守护你的文件。AI 改错或误删时，一键回到任意历史状态。
        </p>
        <div className="flex flex-col gap-[18px]">
          <FeatureRow
            icon={<svg width="16" height="16" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="1.5" strokeLinecap="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" /></svg>}
            title="静默版本保护"
            desc="实时捕捉文件变更，利用 APFS clonefile 异步备份，零额外磁盘占用。"
          />
          <FeatureRow
            icon={<svg width="16" height="16" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M4 6h16M4 12h10M4 18h13" /></svg>}
            title="AI 操作追踪"
            desc="集成 Cursor 与 Claude Code，自动将文件修改聚合为可回溯的任务节点。"
          />
          <FeatureRow
            icon={<svg width="16" height="16" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="1.5" strokeLinecap="round"><path d="M3 12a9 9 0 1 1 2.5 6.2" /><polyline points="3,8 3,12.5 7.5,12.5" /></svg>}
            title="一键倒带"
            desc="任意存档点一键恢复，支持单文件级精确还原。"
          />
        </div>
      </div>
      <Actions>
        <BtnSkip onClick={onSkip}>跳过</BtnSkip>
        <BtnPrimary onClick={onNext}>继续</BtnPrimary>
      </Actions>
    </>
  );
}

function FeatureRow({ icon, title, desc }: { icon: React.ReactNode; title: string; desc: string }) {
  return (
    <div className="flex items-start gap-3">
      <span className="flex-shrink-0 mt-[1px]" style={{ color: "#8e8e93" }}>{icon}</span>
      <div>
        <div className="text-[13px] font-semibold mb-[1px]" style={{ color: "#1d1d1f" }}>{title}</div>
        <div className="text-[12px] leading-[1.45]" style={{ color: "#8e8e93" }}>{desc}</div>
      </div>
    </div>
  );
}

// ─── Step 2: Directories ─────────────────────────────────────────
interface DirOption { path: string; label: string; desc: string; checked: boolean }

function StepDirs({ onNext, onBack }: { onNext: (dirs: string[]) => void; onBack: () => void }) {
  const [dirs, setDirs] = useState<DirOption[]>([]);
  const [customDirs, setCustomDirs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    invoke<string>("get_home_dir")
      .then((home) => {
        setDirs([
          { path: `${home}/Desktop`, label: "桌面", desc: "截图、临时文件、日常使用的文件", checked: false },
          { path: `${home}/Documents`, label: "文稿", desc: "文档、项目、个人资料", checked: false },
          { path: `${home}/Downloads`, label: "下载", desc: "浏览器下载的文件、安装包等", checked: false },
        ]);
      })
      .catch(() => {
        setDirs([
          { path: "~/Desktop", label: "桌面", desc: "截图、临时文件、日常使用的文件", checked: false },
          { path: "~/Documents", label: "文稿", desc: "文档、项目、个人资料", checked: false },
          { path: "~/Downloads", label: "下载", desc: "浏览器下载的文件、安装包等", checked: false },
        ]);
      });
  }, []);

  const toggle = (i: number) => {
    setDirs((prev) => prev.map((d, idx) => idx === i ? { ...d, checked: !d.checked } : d));
    setError(null);
  };

  const pickCustom = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({ directory: true, multiple: true });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      setCustomDirs((prev) => { const m = [...prev]; for (const p of paths) if (!m.includes(p)) m.push(p); return m; });
      setError(null);
    } catch (e) { console.error("dir picker error:", e); }
  };

  const handleNext = async () => {
    const selected = [...dirs.filter((d) => d.checked).map((d) => d.path), ...customDirs];
    if (selected.length === 0) { setError("请至少选择一个目录"); return; }
    setLoading(true); setError(null);
    try { await completeSetup(selected); onNext(selected); }
    catch (e) { setError(String(e)); setLoading(false); }
  };

  const count = dirs.filter((d) => d.checked).length + customDirs.length;

  // SVG icons for dir types
  const dirIcons = [
    <svg key="0" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round"><rect x="3" y="8" width="18" height="11" rx="2"/><path d="M6 8V6a2 2 0 012-2h8a2 2 0 012 2v2"/></svg>,
    <svg key="1" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24" strokeLinecap="round" strokeLinejoin="round"><path d="M4 4h5l2 2h5a2 2 0 012 2v8a2 2 0 01-2 2H4a2 2 0 01-2-2V6a2 2 0 012-2z"/></svg>,
    <svg key="2" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.5" viewBox="0 0 24 24" strokeLinecap="round"><path d="M12 5v10M8 11l4 4 4-4"/><line x1="5" y1="18" x2="19" y2="18"/></svg>,
  ];

  return (
    <>
      <div className="flex-1 max-w-[400px]">
        <h1 className="text-[17px] font-semibold mb-1.5" style={{ color: "#1d1d1f", letterSpacing: "-0.01em" }}>选择保护目录</h1>
        <p className="text-[13px] leading-[1.5] mb-6" style={{ color: "#6e6e73" }}>
          rew 会监控这些目录下的文件变更。选平时放重要文件的地方就好。
        </p>

        {/* Dir list — inset grouped */}
        <div className="rounded-md overflow-hidden mb-2" style={{ border: "0.5px solid rgba(0,0,0,0.08)" }}>
          {dirs.map((d, i) => (
            <div
              key={i}
              onClick={() => toggle(i)}
              className="flex items-center gap-2.5 cursor-default"
              style={{
                padding: "9px 12px",
                borderBottom: i < dirs.length - 1 ? "0.5px solid rgba(0,0,0,0.04)" : "none",
                background: d.checked ? "rgba(0,122,255,0.03)" : "white",
              }}
            >
              {/* Checkbox */}
              <div
                className="w-[13px] h-[13px] rounded-[3px] flex items-center justify-center flex-shrink-0 text-[9px]"
                style={{
                  border: d.checked ? "1px solid #007aff" : "1px solid #c7c7cc",
                  background: d.checked ? "#007aff" : "white",
                  color: d.checked ? "white" : "transparent",
                }}
              >
                &#10003;
              </div>
              {/* Icon */}
              <span className="flex-shrink-0" style={{ color: "#c7c7cc" }}>{dirIcons[i]}</span>
              <div>
                <div className="text-[13px] font-medium" style={{ color: "#1d1d1f" }}>{d.label}</div>
                <div className="text-[11px]" style={{ color: "#aeaeb2" }}>{d.desc}</div>
              </div>
            </div>
          ))}
        </div>

        {/* Custom dirs */}
        {customDirs.map((p) => (
          <div key={p} className="flex items-center gap-2 mb-1 text-[12px]" style={{ color: "#1d1d1f" }}>
            <span style={{ color: "#007aff" }}>&#10003;</span>
            <span className="truncate flex-1">{p.split("/").pop()}</span>
            <button onClick={() => setCustomDirs((prev) => prev.filter((x) => x !== p))} className="text-[11px] border-none bg-transparent cursor-default" style={{ color: "#aeaeb2" }}>移除</button>
          </div>
        ))}

        {/* Add dir — plain text link */}
        <button onClick={pickCustom} className="inline-flex items-center gap-[3px] text-[12px] border-none bg-transparent cursor-default mb-4 py-1" style={{ color: "#007aff" }}>
          + 选择其他目录
        </button>

        {error && <div className="text-[12px] mb-2" style={{ color: "#ff3b30" }}>{error}</div>}
        <div className="text-[11px]" style={{ color: "#c7c7cc" }}>选错了没关系，之后可以在设置中随时调整</div>
      </div>
      <Actions>
        <BtnSkip onClick={onBack}>返回</BtnSkip>
        <BtnPrimary onClick={handleNext} disabled={loading}>
          {loading ? "初始化中..." : count > 0 ? "开始保护" : "请选择目录"}
        </BtnPrimary>
      </Actions>
    </>
  );
}

// ─── Step 3: Scanning ─────────────────────────────────────────────
function StepScanning({ onNext }: { onNext: () => void }) {
  const [progress, setProgress] = useState<ScanProgressInfo | null>(null);
  const [canSkip, setCanSkip] = useState(false);

  useEffect(() => {
    getScanProgress().then(setProgress).catch(console.error);
    const u1 = listen("scan-progress", () => getScanProgress().then(setProgress).catch(console.error));
    const u2 = listen("scan-complete", () => getScanProgress().then(setProgress).catch(console.error));
    const skip = setTimeout(() => setCanSkip(true), 3000);
    const poll = setInterval(() => {
      getScanProgress().then((p) => { setProgress(p); if (p && !p.is_scanning) clearInterval(poll); }).catch(console.error);
    }, 1500);
    return () => { clearTimeout(skip); clearInterval(poll); u1.then((fn) => fn()); u2.then((fn) => fn()); };
  }, []);

  const dirs = progress?.dirs ?? [];
  const totalFiles = dirs.reduce((s, d) => s + d.files_total, 0);
  const doneFiles = dirs.reduce((s, d) => s + d.files_done, 0);
  const pct = totalFiles > 0 ? Math.min((doneFiles / totalFiles) * 100, 100) : 0;
  const isDone = progress !== null && !progress.is_scanning;

  // Auto-advance when scan completes
  useEffect(() => { if (isDone) { const t = setTimeout(onNext, 800); return () => clearTimeout(t); } }, [isDone, onNext]);

  return (
    <>
      <div className="flex-1 max-w-[400px]">
        <h1 className="text-[17px] font-semibold mb-1.5" style={{ color: "#1d1d1f", letterSpacing: "-0.01em" }}>
          {isDone ? "初始化完成" : "正在初始化保护"}
        </h1>
        <p className="text-[13px] leading-[1.5] mb-6" style={{ color: "#6e6e73" }}>
          {isDone
            ? "所有文件已建立保护基线。"
            : "正在扫描目录，为每个文件建立保护基线。完成后即使文件被误删也能恢复。"}
        </p>

        {/* Progress bar */}
        {totalFiles > 0 && (
          <div className="mb-[18px]">
            <div className="flex justify-between text-[11px] mb-1" style={{ color: "#aeaeb2" }}>
              <span>{isDone ? "扫描完成" : `已扫描 ${doneFiles.toLocaleString()} / ${totalFiles.toLocaleString()} 个文件`}</span>
              <span>{Math.round(pct)}%</span>
            </div>
            <div className="h-[2px] rounded-[1px] overflow-hidden" style={{ background: "rgba(0,0,0,0.04)" }}>
              <div className="h-full rounded-[1px] transition-all duration-500" style={{ width: `${pct}%`, background: isDone ? "#34c759" : "#007aff" }} />
            </div>
          </div>
        )}

        {/* Per-dir */}
        {dirs.length > 0 && (
          <div className="mb-4">
            {dirs.map((dir) => (
              <div key={dir.path} className="flex items-center gap-2 py-[5px]">
                <div className="w-[5px] h-[5px] rounded-full flex-shrink-0" style={{ background: dir.status === "complete" ? "#34c759" : dir.status === "scanning" ? "#007aff" : "#ddd" }} />
                <div>
                  <div className="text-[13px] font-medium" style={{ color: "#1d1d1f" }}>{dir.name}</div>
                  <div className="text-[11px]" style={{ color: "#aeaeb2" }}>
                    {dir.status === "complete"
                      ? `${dir.files_done.toLocaleString()} 个文件已保护`
                      : dir.status === "scanning"
                        ? `扫描中 ${dir.percent.toFixed(0)}% · ${dir.files_done.toLocaleString()} / ${dir.files_total.toLocaleString()} 个`
                        : "等待中..."}
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}

        {/* Waiting state */}
        {dirs.length === 0 && !isDone && (
          <div className="flex items-center gap-2 py-6 text-[12px]" style={{ color: "#aeaeb2" }}>
            <span className="animate-spin">◐</span> 正在启动扫描...
          </div>
        )}

        <div className="text-[11px] leading-[1.45]" style={{ color: "#8e8e93" }}>
          使用 APFS clonefile 技术，初次扫描不会额外占用磁盘空间。
        </div>
      </div>
      <Actions>
        {!isDone && canSkip && <BtnSkip onClick={onNext}>在后台继续，先跳过</BtnSkip>}
        {isDone && <BtnPrimary onClick={onNext}>继续</BtnPrimary>}
      </Actions>
    </>
  );
}

// ─── Step 4: Interval ─────────────────────────────────────────────
function StepInterval({ onNext, onBack }: { onNext: () => void; onBack: () => void }) {
  const [selected, setSelected] = useState(600);
  const [saving, setSaving] = useState(false);

  const handleNext = async () => {
    setSaving(true);
    try { await setMonitoringWindow(selected); } catch (e) { console.error(e); }
    finally { setSaving(false); onNext(); }
  };

  return (
    <>
      <div className="flex-1 max-w-[400px]">
        <h1 className="text-[17px] font-semibold mb-1.5" style={{ color: "#1d1d1f", letterSpacing: "-0.01em" }}>自动存档频率</h1>
        <p className="text-[13px] leading-[1.5] mb-6" style={{ color: "#6e6e73" }}>
          按这个频率自动创建文件存档点。频率越高，可回溯的细节越多。
        </p>

        {/* Interval list — inset grouped */}
        <div className="rounded-md overflow-hidden mb-2.5" style={{ border: "0.5px solid rgba(0,0,0,0.08)" }}>
          {WINDOW_OPTIONS.map((opt, i) => (
            <div
              key={opt.secs}
              onClick={() => setSelected(opt.secs)}
              className="flex items-center gap-2.5 cursor-default"
              style={{
                padding: "10px 12px",
                borderBottom: i < WINDOW_OPTIONS.length - 1 ? "0.5px solid rgba(0,0,0,0.04)" : "none",
                background: selected === opt.secs ? "rgba(0,122,255,0.03)" : "white",
              }}
            >
              {/* Radio */}
              <div
                className="w-[14px] h-[14px] rounded-full flex items-center justify-center flex-shrink-0"
                style={{ border: selected === opt.secs ? "1px solid #007aff" : "1px solid #c7c7cc" }}
              >
                {selected === opt.secs && <div className="w-[6px] h-[6px] rounded-full" style={{ background: "#007aff" }} />}
              </div>
              <div>
                <div>
                  <span className="text-[13px] font-medium" style={{ color: "#1d1d1f" }}>{opt.label}</span>
                  {opt.tag && <span className="text-[10px] ml-1" style={{ color: "#aeaeb2" }}>{opt.tag}</span>}
                </div>
                <div className="text-[11px] mt-[1px] leading-[1.4]" style={{ color: "#8e8e93" }}>{opt.desc}</div>
              </div>
            </div>
          ))}
        </div>

        <div className="text-[11px] leading-[1.45]" style={{ color: "#8e8e93" }}>
          <b style={{ fontWeight: 500, color: "#6e6e73" }}>AI 任务不受此频率影响</b> — 操作完成后会立即创建存档。
        </div>
      </div>
      <Actions>
        <BtnSkip onClick={onBack}>返回</BtnSkip>
        <BtnPrimary onClick={handleNext} disabled={saving}>完成</BtnPrimary>
      </Actions>
    </>
  );
}

// ─── Step 5: Done ─────────────────────────────────────────────────
function StepDone({ onComplete }: { onComplete: () => void }) {
  const tips = [
    { icon: <svg width="14" height="14" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="1.5" strokeLinecap="round"><path d="M4 6h16M4 12h10M4 18h13"/></svg>, text: "AI 工具操作完成后自动生成存档，时间线里可以看到每次操作了哪些文件。" },
    { icon: <svg width="14" height="14" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="1.5" strokeLinecap="round"><circle cx="12" cy="12" r="9"/><polyline points="12,7 12,12 15.5,14"/></svg>, text: "按设置的频率，rew 定时把文件变更打成存档包，随时可以倒带。" },
    { icon: <svg width="14" height="14" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="1.5" strokeLinecap="round"><path d="M3 12a9 9 0 1 1 2.5 6.2"/><polyline points="3,8 3,12.5 7.5,12.5"/></svg>, text: "文件被改错了？点击存档记录 → 选文件 → 倒带，一键恢复。" },
    { icon: <svg width="14" height="14" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="1.5" strokeLinecap="round"><circle cx="12" cy="12" r="3"/><path d="M12 3v3M12 18v3M3 12h3M18 12h3"/></svg>, text: "设置中可以添加更多保护目录，或调整存档频率。" },
  ];

  return (
    <>
      <div className="flex-1 max-w-[400px]">
        <h1 className="text-[17px] font-semibold mb-1.5" style={{ color: "#1d1d1f", letterSpacing: "-0.01em" }}>设置完成</h1>
        <p className="text-[13px] leading-[1.5] mb-6" style={{ color: "#6e6e73" }}>
          rew 已在后台运行，不需要额外操作。
        </p>
        <div className="flex flex-col gap-3.5">
          {tips.map((t, i) => (
            <div key={i} className="flex items-start gap-2.5">
              <span className="flex-shrink-0 mt-[1px]" style={{ color: "#c7c7cc" }}>{t.icon}</span>
              <div className="text-[12px] leading-[1.5]" style={{ color: "#8e8e93" }}>{t.text}</div>
            </div>
          ))}
        </div>
      </div>
      <Actions>
        <BtnPrimary onClick={onComplete}>进入 rew</BtnPrimary>
      </Actions>
    </>
  );
}

// ─── Root ─────────────────────────────────────────────────────────
export default function SetupWizard({ onComplete }: Props) {
  const [step, setStep] = useState<Step>("welcome");
  const [selectedDirs, setSelectedDirs] = useState<string[]>([]);

  return (
    <div className="flex h-screen w-screen overflow-hidden" style={{ background: "rgba(255,255,255,0.88)", backdropFilter: "blur(60px) saturate(200%)", WebkitBackdropFilter: "blur(60px) saturate(200%)" }}>
      <Sidebar current={step} />
      <main className="flex-1 flex flex-col overflow-y-auto" style={{ padding: "40px 48px 28px" }}>
        {step === "welcome" && (
          <StepWelcome onNext={() => setStep("dirs")} onSkip={onComplete} />
        )}
        {step === "dirs" && (
          <StepDirs
            onNext={(dirs) => { setSelectedDirs(dirs); setStep("scanning"); }}
            onBack={() => setStep("welcome")}
          />
        )}
        {step === "scanning" && (
          <StepScanning onNext={() => setStep("interval")} />
        )}
        {step === "interval" && (
          <StepInterval onNext={() => setStep("done")} onBack={() => setStep("scanning")} />
        )}
        {step === "done" && <StepDone onComplete={onComplete} />}
      </main>
    </div>
  );
}
