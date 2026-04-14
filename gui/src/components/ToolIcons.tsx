/**
 * AI tool brand icons — shared across Sidebar, Timeline, and Settings.
 * Real brand icons extracted from installed applications.
 */

import cursorIcon from "../assets/icons/cursor.png";
import codebuddyIcon from "../assets/icons/codebuddy.png";
import workbuddyIcon from "../assets/icons/workbuddy.png";

/** Renders a PNG brand icon inside a rounded container */
function PngBrandIcon({
  src,
  size = 20,
  alt,
}: {
  src: string;
  size?: number;
  alt: string;
}) {
  return (
    <img
      src={src}
      alt={alt}
      width={size}
      height={size}
      className="rounded-md flex-shrink-0"
      style={{ width: size, height: size }}
      draggable={false}
    />
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
      return <PngBrandIcon src={cursorIcon} size={size} alt="Cursor" />;
    case "claude-code":
    case "claude_code":
      return <ClaudeCodeBrandIcon size={size} />;
    case "codebuddy":
      return <PngBrandIcon src={codebuddyIcon} size={size} alt="CodeBuddy" />;
    case "workbuddy":
      return <PngBrandIcon src={workbuddyIcon} size={size} alt="WorkBuddy" />;
    default:
      return <GenericAiIcon size={size} />;
  }
}
