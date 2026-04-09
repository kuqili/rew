/**
 * DiffViewer — renders a unified diff with proper old/new line numbers.
 *
 * Parses the `@@ -start,count +start,count @@` hunk headers to track
 * exact line numbers, giving a GitHub-style diff experience.
 */
interface Props {
  diffText: string | null;
  loading?: boolean;
}

interface DiffLine {
  type: "header" | "hunk" | "add" | "del" | "ctx";
  content: string;
  oldLineNo: number | null;
  newLineNo: number | null;
}

function parseDiff(text: string): DiffLine[] {
  const lines: DiffLine[] = [];
  let oldLine = 0;
  let newLine = 0;

  for (const raw of text.split("\n")) {
    if (raw === "") continue;

    if (raw.startsWith("--- ") || raw.startsWith("+++ ")) {
      lines.push({ type: "header", content: raw, oldLineNo: null, newLineNo: null });
      continue;
    }

    if (raw.startsWith("@@")) {
      // @@ -start[,count] +start[,count] @@
      const m = raw.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (m) {
        oldLine = parseInt(m[1], 10);
        newLine = parseInt(m[2], 10);
      }
      lines.push({ type: "hunk", content: raw, oldLineNo: null, newLineNo: null });
      continue;
    }

    if (raw.startsWith("-")) {
      lines.push({ type: "del", content: raw.slice(1), oldLineNo: oldLine++, newLineNo: null });
      continue;
    }

    if (raw.startsWith("+")) {
      lines.push({ type: "add", content: raw.slice(1), oldLineNo: null, newLineNo: newLine++ });
      continue;
    }

    // Context line (space prefix or unrecognised)
    const content = raw.startsWith(" ") ? raw.slice(1) : raw;
    lines.push({ type: "ctx", content, oldLineNo: oldLine++, newLineNo: newLine++ });
  }

  return lines;
}

export default function DiffViewer({ diffText, loading }: Props) {
  if (loading) {
    return (
      <div className="px-6 py-3 text-ink-muted text-2xs flex items-center gap-2">
        <span className="animate-spin inline-block">◐</span> 加载 diff...
      </div>
    );
  }

  if (!diffText) {
    return (
      <div className="px-6 py-3 text-ink-muted text-2xs text-center">
        二进制文件或无可用 diff
      </div>
    );
  }

  // Simple notice messages (not actual diff)
  if (!diffText.includes("@@") && !diffText.startsWith("---")) {
    return (
      <div className="px-6 py-3 text-ink-muted text-2xs text-center">{diffText.trim()}</div>
    );
  }

  const lines = parseDiff(diffText);

  return (
    <div className="overflow-x-auto max-h-[400px] overflow-y-auto text-[11px] font-mono leading-[1.55]">
      <table className="w-full border-collapse">
        <tbody>
          {lines.map((line, i) => {
            if (line.type === "header") {
              return (
                <tr key={i} className="bg-surface-secondary text-ink-muted">
                  <td colSpan={3} className="px-4 py-0.5 select-none">{line.content}</td>
                </tr>
              );
            }

            if (line.type === "hunk") {
              return (
                <tr key={i} className="bg-st-blue/5 text-st-blue">
                  <td colSpan={3} className="px-4 py-0.5 select-none">{line.content}</td>
                </tr>
              );
            }

            const isAdd = line.type === "add";
            const isDel = line.type === "del";

            const rowBg = isAdd
              ? "bg-[#e6ffec]"
              : isDel
                ? "bg-[#ffebe9]"
                : "";

            const sign = isAdd ? "+" : isDel ? "−" : " ";
            const signColor = isAdd
              ? "text-status-green"
              : isDel
                ? "text-status-red"
                : "text-ink-faint";

            return (
              <tr key={i} className={rowBg}>
                {/* Old line number */}
                <td
                  className="w-[44px] text-right pr-2 pl-2 border-r border-surface-border/30 text-ink-faint select-none tabular-nums"
                  style={{ minWidth: 44 }}
                >
                  {line.oldLineNo ?? ""}
                </td>
                {/* New line number */}
                <td
                  className="w-[44px] text-right pr-2 pl-2 border-r border-surface-border/30 text-ink-faint select-none tabular-nums"
                  style={{ minWidth: 44 }}
                >
                  {line.newLineNo ?? ""}
                </td>
                {/* Sign + content */}
                <td className="pl-3 pr-4 whitespace-pre">
                  <span className={`mr-2 ${signColor} select-none`}>{sign}</span>
                  <span className="text-ink">{line.content}</span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
