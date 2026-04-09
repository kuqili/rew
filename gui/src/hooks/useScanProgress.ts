import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { getScanProgress, type ScanProgressInfo } from "../lib/tauri";

export function useScanProgress() {
  const [progress, setProgress] = useState<ScanProgressInfo | null>(null);

  useEffect(() => {
    // Initial fetch
    getScanProgress().then(setProgress).catch(console.error);

    // Listen for real-time updates
    const unlistenProgress = listen("scan-progress", () => {
      getScanProgress().then(setProgress).catch(console.error);
    });

    const unlistenComplete = listen("scan-complete", () => {
      getScanProgress().then(setProgress).catch(console.error);
    });

    // Fallback poll every 2s while scanning
    const timer = setInterval(() => {
      getScanProgress()
        .then((p) => {
          setProgress(p);
          if (p && !p.is_scanning) clearInterval(timer);
        })
        .catch(console.error);
    }, 2000);

    return () => {
      clearInterval(timer);
      unlistenProgress.then((fn) => fn());
      unlistenComplete.then((fn) => fn());
    };
  }, []);

  return progress;
}
