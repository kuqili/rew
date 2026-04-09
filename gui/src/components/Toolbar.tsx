interface Props {
  onBack?: () => void;
  onSettings?: () => void;
}

export default function Toolbar({ onBack, onSettings }: Props) {
  return (
    <div
      data-tauri-drag-region
      className="h-[52px] min-h-[52px] bg-surface-secondary border-b border-surface-border flex items-center px-3 gap-1"
    >
      {/* Back button when in detail/settings view */}
      {onBack && (
        <button
          onClick={onBack}
          className="toolbar-btn"
        >
          <span className="toolbar-icon">←</span>
          <span>返回</span>
        </button>
      )}

      {/* Spacer for drag */}
      <div className="flex-1" data-tauri-drag-region />

      {/* Right-side toolbar buttons */}
      <button
        onClick={onSettings}
        className="toolbar-btn"
      >
        <span className="toolbar-icon">⚙️</span>
        <span>设置</span>
      </button>
    </div>
  );
}
