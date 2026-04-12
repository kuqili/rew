import { useState, useEffect } from "react";
import { completeSetup, setMonitoringWindow, getScanProgress, type ScanProgressInfo } from "../lib/tauri";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Props {
  onComplete: () => void;
}

type Step = "welcome" | "dirs" | "scanning" | "interval" | "done";

const STEPS: Step[] = ["welcome", "dirs", "scanning", "interval", "done"];

const STEP_LABELS: Record<Step, string> = {
  welcome: "欢迎",
  dirs: "保护目录",
  scanning: "初始化",
  interval: "存档频率",
  done: "完成",
};

const WINDOW_OPTIONS = [
  {
    secs: 300,
    label: "每 5 分钟",
    sublabel: "推荐 · 重度 AI 用户",
    desc: "每隔 5 分钟生成一次存档点，适合频繁使用 AI 改动文件的场景。",
    icon: "⚡",
  },
  {
    secs: 600,
    label: "每 10 分钟",
    sublabel: "均衡选择",
    desc: "每隔 10 分钟存一次，存档数量适中，磁盘占用少，适合大多数用户。",
    icon: "⚖️",
  },
  {
    secs: 1800,
    label: "每 30 分钟",
    sublabel: "轻度使用",
    desc: "适合偶尔使用 AI 工具、不需要密集保护的场景。",
    icon: "🌿",
  },
  {
    secs: 3600,
    label: "每 1 小时",
    sublabel: "最低频率",
    desc: "每小时存一次，磁盘占用极少，但相邻两个存档之间的变更较多。",
    icon: "🐌",
  },
];

// ─── Step indicator ───────────────────────────────────────────────────────────

function StepIndicator({ current }: { current: Step }) {
  const visibleSteps: Step[] = ["dirs", "scanning", "interval", "done"];
  return (
    <div className="flex items-center justify-center gap-1.5 mb-7">
      {visibleSteps.map((s, i) => {
        const cIdx = visibleSteps.indexOf(current);
        const done = i < cIdx;
        const active = s === current;
        return (
          <div key={s} className="flex items-center gap-1.5">
            {i > 0 && (
              <div className={`w-6 h-px ${done || active ? "bg-safe-blue" : "bg-surface-border"}`} />
            )}
            <div className={`w-5 h-5 rounded-full flex items-center justify-center text-[10px] font-semibold transition-all ${
              done
                ? "bg-safe-blue text-white"
                : active
                  ? "bg-safe-blue text-white ring-2 ring-safe-blue/30"
                  : "bg-surface-primary text-ink-muted border border-surface-border"
            }`}>
              {done ? "✓" : i + 1}
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ─── Step 1: Welcome ─────────────────────────────────────────────────────────

function StepWelcome({ onNext }: { onNext: () => void }) {
  return (
    <div className="text-center">
      <div className="text-5xl mb-4">🛡️</div>
      <h1 className="text-[18px] font-semibold text-ink mb-2">欢迎使用 rew</h1>
      <p className="text-[13px] text-ink-secondary leading-relaxed mb-5">
        AI 工具改文件很快，快到你来不及反应。<br />
        rew 默默在后台给你的文件做存档，<br />
        万一 AI 改错了、删错了——随时读档回去。
      </p>

      <div className="space-y-2 mb-6 text-left">
        <FeatureRow icon="📸" title="自动存档" desc="AI 每次操作完成后自动创建存档，不需要你手动操作" />
        <FeatureRow icon="🔍" title="查看变更" desc="清晰展示每次 AI 操作了哪些文件、改了什么" />
        <FeatureRow icon="⏮️" title="一键读档" desc="任意一个存档点都可以一键恢复，像游戏读档一样简单" />
        <FeatureRow icon="🚨" title="危险拦截" desc="检测到大批量删除等危险操作时主动告警" />
      </div>

      <button
        onClick={onNext}
        className="w-full py-2.5 rounded-xl bg-safe-blue text-white text-[13px] font-medium hover:opacity-90 transition-opacity"
      >
        开始设置 →
      </button>
    </div>
  );
}

function FeatureRow({ icon, title, desc }: { icon: string; title: string; desc: string }) {
  return (
    <div className="flex items-start gap-3 px-3 py-2.5 bg-surface-primary/60 rounded-lg">
      <span className="text-lg mt-0.5 flex-shrink-0">{icon}</span>
      <div>
        <div className="text-[12px] font-semibold text-ink">{title}</div>
        <div className="text-[11px] text-ink-muted leading-relaxed">{desc}</div>
      </div>
    </div>
  );
}

// ─── Step 2: Choose directories ───────────────────────────────────────────────

interface DirOption {
  path: string;
  label: string;
  description: string;
  icon: string;
  checked: boolean;
}

function StepDirs({ onNext }: { onNext: (dirs: string[]) => void }) {
  const [dirs, setDirs] = useState<DirOption[]>([]);
  const [customDirs, setCustomDirs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    invoke<string>("get_home_dir")
      .then((home) => {
        setDirs([
          { path: `${home}/Desktop`, label: "桌面", description: "截图、临时文件、日常使用的文件", icon: "🖥️", checked: false },
          { path: `${home}/Documents`, label: "文稿", description: "文档、项目、个人资料", icon: "📄", checked: false },
          { path: `${home}/Downloads`, label: "下载", description: "浏览器下载的文件、安装包等", icon: "⬇️", checked: false },
        ]);
      })
      .catch(() => {
        setDirs([
          { path: "~/Desktop", label: "桌面", description: "截图、临时文件、日常使用的文件", icon: "🖥️", checked: false },
          { path: "~/Documents", label: "文稿", description: "文档、项目、个人资料", icon: "📄", checked: false },
          { path: "~/Downloads", label: "下载", description: "浏览器下载的文件、安装包等", icon: "⬇️", checked: false },
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
      setCustomDirs((prev) => {
        const merged = [...prev];
        for (const p of paths) if (!merged.includes(p)) merged.push(p);
        return merged;
      });
      setError(null);
    } catch (e) {
      console.error("dir picker error:", e);
    }
  };

  const handleNext = async () => {
    const selected = [
      ...dirs.filter((d) => d.checked).map((d) => d.path),
      ...customDirs,
    ];
    if (selected.length === 0) {
      setError("请至少选择一个目录，rew 才知道保护哪里");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      await completeSetup(selected);
      onNext(selected);
    } catch (e) {
      setError(String(e));
      setLoading(false);
    }
  };

  const selectedCount = dirs.filter((d) => d.checked).length + customDirs.length;

  return (
    <div>
      <StepIndicator current="dirs" />
      <div className="text-center mb-5">
        <h2 className="text-[16px] font-semibold text-ink mb-1">选择保护目录</h2>
        <p className="text-[12px] text-ink-muted leading-relaxed">
          rew 会监控这些目录下的文件变更。<br />
          选你平时放重要文件的地方就好。
        </p>
      </div>

      <div className="space-y-1.5 mb-3">
        {dirs.map((d, i) => (
          <label key={i} className={`flex items-center gap-3 px-3 py-2.5 rounded-xl cursor-pointer transition-all border ${
            d.checked ? "bg-safe-blue/5 border-safe-blue/40" : "border-surface-border hover:bg-surface-primary/60"
          }`}>
            <input
              type="checkbox"
              checked={d.checked}
              onChange={() => toggle(i)}
              className="w-4 h-4 rounded accent-safe-blue flex-shrink-0"
            />
            <span className="text-xl flex-shrink-0">{d.icon}</span>
            <div className="flex-1 min-w-0">
              <div className="text-[13px] font-medium text-ink">{d.label}</div>
              <div className="text-[11px] text-ink-muted">{d.description}</div>
            </div>
            {d.checked && <span className="text-safe-blue text-[11px] flex-shrink-0">✓ 已选</span>}
          </label>
        ))}
      </div>

      {/* Custom dirs */}
      {customDirs.map((p) => (
        <div key={p} className="flex items-center gap-2 px-3 py-2 mb-1.5 bg-safe-blue/5 border border-safe-blue/30 rounded-xl">
          <span className="text-base flex-shrink-0">📁</span>
          <span className="flex-1 text-[12px] text-ink truncate">{p.split("/").pop()}</span>
          <span className="text-[10px] text-ink-muted truncate max-w-[120px] hidden sm:block">{p}</span>
          <button onClick={() => setCustomDirs((prev) => prev.filter((x) => x !== p))}
            className="text-ink-faint hover:text-status-red text-[11px] flex-shrink-0 ml-1">移除</button>
        </div>
      ))}

      <button onClick={pickCustom}
        className="w-full py-2 mb-4 rounded-xl border border-dashed border-surface-border text-[12px] text-ink-muted hover:border-safe-blue hover:text-safe-blue transition-colors">
        + 选择其他目录
      </button>

      {error && (
        <div className="mb-3 px-3 py-2 bg-status-red-bg text-status-red text-[12px] rounded-lg">{error}</div>
      )}

      <button
        onClick={handleNext}
        disabled={loading}
        className="w-full py-2.5 rounded-xl bg-safe-blue text-white text-[13px] font-medium hover:opacity-90 disabled:opacity-40 transition-opacity"
      >
        {loading
          ? <span className="flex items-center justify-center gap-2"><span className="animate-spin">◐</span>正在初始化...</span>
          : selectedCount > 0
            ? `开始保护 ${selectedCount} 个目录 →`
            : "请选择至少一个目录"
        }
      </button>

      <p className="text-center text-[10px] text-ink-faint mt-3">
        选错了没关系，之后可以在设置中随时调整
      </p>
    </div>
  );
}

// ─── Step 3: Scanning ─────────────────────────────────────────────────────────

function StepScanning({ selectedDirs, onNext }: { selectedDirs: string[]; onNext: () => void }) {
  const [progress, setProgress] = useState<ScanProgressInfo | null>(null);
  const [canSkip, setCanSkip] = useState(false);

  useEffect(() => {
    getScanProgress().then(setProgress).catch(console.error);

    const unlistenProgress = listen("scan-progress", () => {
      getScanProgress().then(setProgress).catch(console.error);
    });
    const unlistenComplete = listen("scan-complete", () => {
      getScanProgress().then(setProgress).catch(console.error);
    });

    // Allow skip after 3s so user isn't trapped
    const skipTimer = setTimeout(() => setCanSkip(true), 3000);

    const poll = setInterval(() => {
      getScanProgress().then((p) => {
        setProgress(p);
        if (p && !p.is_scanning) clearInterval(poll);
      }).catch(console.error);
    }, 1500);

    return () => {
      clearTimeout(skipTimer);
      clearInterval(poll);
      unlistenProgress.then((fn) => fn());
      unlistenComplete.then((fn) => fn());
    };
  }, []);

  const dirs = progress?.dirs ?? [];
  const totalFiles = dirs.reduce((s, d) => s + d.files_total, 0);
  const doneFiles = dirs.reduce((s, d) => s + d.files_done, 0);
  const overallPct = totalFiles > 0 ? Math.min((doneFiles / totalFiles) * 100, 100) : 0;
  const isDone = progress !== null && !progress.is_scanning;
  const isScanning = progress?.is_scanning ?? true;

  return (
    <div>
      <StepIndicator current="scanning" />
      <div className="text-center mb-5">
        <div className={`text-4xl mb-3 ${isScanning ? "animate-pulse" : ""}`}>
          {isDone ? "✅" : "🔍"}
        </div>
        <h2 className="text-[16px] font-semibold text-ink mb-1">
          {isDone ? "初始化完成" : "正在初始化保护..."}
        </h2>
        <p className="text-[12px] text-ink-muted leading-relaxed">
          {isDone
            ? "rew 已完成初始扫描，所有文件都已建立保护基线。"
            : "rew 正在扫描你选择的目录，为每个文件建立保护基线。\n扫描完成后，即使 AI 误删文件也能恢复。"}
        </p>
      </div>

      {/* Overall progress */}
      {totalFiles > 0 && (
        <div className="mb-4">
          <div className="flex items-center justify-between text-[11px] text-ink-muted mb-1.5">
            <span>{isDone ? "✓ 全部完成" : `已扫描 ${doneFiles.toLocaleString()} / ${totalFiles.toLocaleString()} 个文件`}</span>
            <span>{Math.round(overallPct)}%</span>
          </div>
          <div className="w-full h-2 bg-surface-primary rounded-full overflow-hidden">
            <div
              className={`h-full rounded-full transition-all duration-500 ${isDone ? "bg-success-green" : "bg-safe-blue"}`}
              style={{ width: `${overallPct}%` }}
            />
          </div>
        </div>
      )}

      {/* Per-dir status */}
      {dirs.length > 0 && (
        <div className="space-y-1.5 mb-5">
          {dirs.map((dir) => (
            <div key={dir.path} className="flex items-center gap-3 px-3 py-2 bg-surface-primary/60 rounded-lg">
              <span className="text-base flex-shrink-0">
                {dir.status === "complete" ? "✅" : dir.status === "scanning" ? "🔄" : "⏳"}
              </span>
              <div className="flex-1 min-w-0">
                <div className="text-[12px] font-medium text-ink truncate">{dir.name}</div>
                <div className="text-[10px] text-ink-muted">
                  {dir.status === "complete"
                    ? `${dir.files_done.toLocaleString()} 个文件已保护`
                    : dir.status === "scanning"
                      ? `扫描中 ${dir.percent.toFixed(0)}% · ${dir.files_done.toLocaleString()} / ${dir.files_total.toLocaleString()} 个`
                      : "等待中..."}
                </div>
              </div>
              {dir.status === "scanning" && (
                <div className="w-12 h-1 bg-surface-border rounded-full overflow-hidden flex-shrink-0">
                  <div className="h-full bg-safe-blue rounded-full" style={{ width: `${Math.min(dir.percent, 100)}%` }} />
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* No progress yet */}
      {dirs.length === 0 && !isDone && (
        <div className="flex items-center justify-center gap-2 py-6 text-[12px] text-ink-muted">
          <span className="animate-spin">◐</span>
          <span>正在启动扫描...</span>
        </div>
      )}

      <div className="space-y-2">
        {isDone && (
          <button
            onClick={onNext}
            className="w-full py-2.5 rounded-xl bg-safe-blue text-white text-[13px] font-medium hover:opacity-90 transition-opacity"
          >
            继续设置 →
          </button>
        )}
        {!isDone && canSkip && (
          <button
            onClick={onNext}
            className="w-full py-2 rounded-xl border border-surface-border text-[12px] text-ink-muted hover:text-ink hover:border-ink-muted transition-colors"
          >
            在后台继续扫描，先跳过
          </button>
        )}
      </div>

      {!isDone && (
        <p className="text-center text-[10px] text-ink-faint mt-3">
          使用 APFS clonefile 技术，初次扫描不会额外占用磁盘空间
        </p>
      )}
    </div>
  );
}

// ─── Step 4: Archive interval ─────────────────────────────────────────────────

function StepInterval({ onNext }: { onNext: () => void }) {
  const [selected, setSelected] = useState(600);
  const [saving, setSaving] = useState(false);

  const handleNext = async () => {
    setSaving(true);
    try {
      await setMonitoringWindow(selected);
    } catch (e) {
      console.error("set interval failed:", e);
    } finally {
      setSaving(false);
      onNext();
    }
  };

  return (
    <div>
      <StepIndicator current="interval" />
      <div className="text-center mb-5">
        <h2 className="text-[16px] font-semibold text-ink mb-1">设置自动存档频率</h2>
        <p className="text-[12px] text-ink-muted leading-relaxed">
          rew 会按这个频率自动给你的文件创建存档点。<br />
          存档点越密，可以回溯的细节越多。
        </p>
      </div>

      <div className="space-y-2 mb-5">
        {WINDOW_OPTIONS.map((opt) => (
          <button
            key={opt.secs}
            onClick={() => setSelected(opt.secs)}
            className={`w-full flex items-start gap-3 px-4 py-3 rounded-xl border text-left transition-all ${
              selected === opt.secs
                ? "bg-safe-blue/5 border-safe-blue/50 ring-1 ring-safe-blue/20"
                : "border-surface-border hover:border-safe-blue/30 hover:bg-surface-primary/40"
            }`}
          >
            <span className="text-xl flex-shrink-0 mt-0.5">{opt.icon}</span>
            <div className="flex-1 min-w-0">
              <div className="flex items-baseline gap-2">
                <span className="text-[13px] font-semibold text-ink">{opt.label}</span>
                <span className={`text-[10px] px-1.5 py-0.5 rounded-full ${
                  selected === opt.secs ? "bg-safe-blue text-white" : "bg-surface-primary text-ink-muted"
                }`}>{opt.sublabel}</span>
              </div>
              <div className="text-[11px] text-ink-muted mt-0.5 leading-relaxed">{opt.desc}</div>
            </div>
            {selected === opt.secs && (
              <span className="text-safe-blue text-[13px] flex-shrink-0 mt-1">✓</span>
            )}
          </button>
        ))}
      </div>

      <div className="px-3 py-2.5 bg-surface-primary/60 rounded-lg mb-4 text-[11px] text-ink-muted leading-relaxed">
        💡 <b>AI 任务期间不受此频率影响</b>——AI 工具（Cursor、Claude Code 等）操作完成后会<b>立即</b>创建存档，不等计时器。
      </div>

      <button
        onClick={handleNext}
        disabled={saving}
        className="w-full py-2.5 rounded-xl bg-safe-blue text-white text-[13px] font-medium hover:opacity-90 disabled:opacity-40 transition-opacity"
      >
        {saving ? <span className="flex items-center justify-center gap-2"><span className="animate-spin">◐</span>保存中...</span> : "确认，完成设置 →"}
      </button>

      <p className="text-center text-[10px] text-ink-faint mt-3">
        之后可以在设置 → 存档设置中随时修改
      </p>
    </div>
  );
}

// ─── Step 5: Done ─────────────────────────────────────────────────────────────

function StepDone({ onComplete }: { onComplete: () => void }) {
  return (
    <div className="text-center">
      <StepIndicator current="done" />
      <div className="text-5xl mb-4">🎉</div>
      <h2 className="text-[16px] font-semibold text-ink mb-2">一切就绪！</h2>
      <p className="text-[12px] text-ink-muted leading-relaxed mb-6">
        rew 已在后台默默保护你的文件。<br />
        你不需要做任何额外操作。
      </p>

      <div className="space-y-2 mb-6 text-left">
        <TipRow icon="🤖" text="用 AI 工具时，操作完成后自动生成存档，时间线里可以看到每次操作了哪些文件" />
        <TipRow icon="🕐" text="按你设置的频率，rew 会定时把文件变更打成一个存档包，随时可以读档" />
        <TipRow icon="⏮️" text="发现文件被改错了？点击存档记录 → 选文件 → 「读档」，一键恢复" />
        <TipRow icon="⚙️" text="右上角设置可以添加更多保护目录，或调整存档频率" />
      </div>

      <button
        onClick={onComplete}
        className="w-full py-2.5 rounded-xl bg-safe-blue text-white text-[13px] font-medium hover:opacity-90 transition-opacity"
      >
        进入 rew →
      </button>
    </div>
  );
}

function TipRow({ icon, text }: { icon: string; text: string }) {
  return (
    <div className="flex items-start gap-3 px-3 py-2.5 bg-surface-primary/60 rounded-lg">
      <span className="text-base flex-shrink-0 mt-0.5">{icon}</span>
      <p className="text-[11px] text-ink-secondary leading-relaxed">{text}</p>
    </div>
  );
}

// ─── Root wizard ──────────────────────────────────────────────────────────────

export default function SetupWizard({ onComplete }: Props) {
  const [step, setStep] = useState<Step>("welcome");
  const [selectedDirs, setSelectedDirs] = useState<string[]>([]);

  return (
    <div className="flex items-center justify-center h-screen bg-gradient-to-br from-surface-primary to-white p-4">
      <div className="bg-white rounded-2xl shadow-panel border border-surface-border p-8 w-[460px] max-h-[90vh] overflow-y-auto">
        {step === "welcome" && (
          <StepWelcome onNext={() => setStep("dirs")} />
        )}
        {step === "dirs" && (
          <StepDirs
            onNext={(dirs) => {
              setSelectedDirs(dirs);
              setStep("scanning");
            }}
          />
        )}
        {step === "scanning" && (
          <StepScanning
            selectedDirs={selectedDirs}
            onNext={() => setStep("interval")}
          />
        )}
        {step === "interval" && (
          <StepInterval onNext={() => setStep("done")} />
        )}
        {step === "done" && (
          <StepDone onComplete={onComplete} />
        )}
      </div>
    </div>
  );
}
