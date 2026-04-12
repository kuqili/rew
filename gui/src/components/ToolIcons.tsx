/**
 * AI tool brand icons — shared across Sidebar, Timeline, and Settings.
 * Each icon uses the tool's official brand color inside a rounded square.
 */

/** Cursor — purple triangle/pointer */
export function CursorBrandIcon({ size = 20 }: { size?: number }) {
  return (
    <span
      className="inline-flex items-center justify-center rounded-md flex-shrink-0"
      style={{ width: size, height: size, background: "#7c3aed" }}
    >
      <svg width={size * 0.55} height={size * 0.55} viewBox="0 0 12 12" fill="none">
        <path d="M2.5 1L10 6L2.5 11V1Z" fill="white" />
      </svg>
    </span>
  );
}

/** Claude Code — orange starburst */
export function ClaudeCodeBrandIcon({ size = 20 }: { size?: number }) {
  return (
    <span
      className="inline-flex items-center justify-center rounded-md flex-shrink-0"
      style={{ width: size, height: size, background: "#C15F3C" }}
    >
      <svg width={size * 0.6} height={size * 0.6} viewBox="0 0 12 12" fill="none">
        <circle cx="6" cy="6" r="2" fill="white" />
        <path d="M6 1V3.5M6 8.5V11M1 6H3.5M8.5 6H11M2.5 2.5L4.2 4.2M7.8 7.8L9.5 9.5M9.5 2.5L7.8 4.2M4.2 7.8L2.5 9.5" stroke="white" strokeWidth="1.2" strokeLinecap="round" />
      </svg>
    </span>
  );
}

/** Generic AI tool icon */
export function GenericAiIcon({ size = 20 }: { size?: number }) {
  return (
    <span
      className="inline-flex items-center justify-center rounded-md flex-shrink-0 bg-ai-purple"
      style={{ width: size, height: size }}
    >
      <svg width={size * 0.55} height={size * 0.55} viewBox="0 0 12 12" fill="none">
        <path d="M6 1L7.5 4.5L11 6L7.5 7.5L6 11L4.5 7.5L1 6L4.5 4.5L6 1Z" fill="white" />
      </svg>
    </span>
  );
}

/** Returns the appropriate brand icon for a tool ID */
export function getToolBrandIcon(toolId: string, size = 20): React.ReactNode {
  const normalized = toolId.toLowerCase().replace(/_/g, "-");
  switch (normalized) {
    case "cursor":
      return <CursorBrandIcon size={size} />;
    case "claude-code":
    case "claude_code":
      return <ClaudeCodeBrandIcon size={size} />;
    default:
      return <GenericAiIcon size={size} />;
  }
}
