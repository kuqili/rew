import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { getBatchProgress } from "../lib/tauri";

export type BatchPhase = "planning" | "writing";

export interface BatchProgressState {
  active: boolean;
  processed: number;
  total: number;
  taskId: string | null;
  phase: BatchPhase;
}

const INITIAL_STATE: BatchProgressState = {
  active: false,
  processed: 0,
  total: 0,
  taskId: null,
  phase: "writing",
};

export function useBatchProgress() {
  const [progress, setProgress] = useState<BatchProgressState>(INITIAL_STATE);

  useEffect(() => {
    // On mount, poll the backend to recover any in-progress batch
    // (handles the case where the window was closed and reopened mid-batch).
    getBatchProgress()
      .then((p) => {
        if (p.is_running) {
          setProgress({
            active: true,
            processed: p.processed_files,
            total: p.total_files,
            taskId: p.task_id,
            phase: "writing",
          });
        }
      })
      .catch(console.error);

    const unlistenStarted = listen<{
      file_count: number;
      task_id: string;
      phase: BatchPhase;
    }>("large-batch-started", (event) => {
      setProgress({
        active: true,
        processed: 0,
        total: event.payload.file_count,
        taskId: event.payload.task_id,
        phase: event.payload.phase ?? "planning",
      });
    });

    const unlistenProgress = listen<{
      processed: number;
      total: number;
      task_id: string;
      phase: BatchPhase;
    }>("large-batch-progress", (event) => {
      setProgress((prev) => {
        const phaseChanged = prev.phase !== event.payload.phase;
        return {
          ...prev,
          active: true,
          // When the phase switches (planning → writing), reset processed to 0
          // so the progress bar advances forward rather than jumping backward.
          processed: phaseChanged ? 0 : event.payload.processed,
          total: event.payload.total,
          taskId: event.payload.task_id,
          phase: event.payload.phase,
        };
      });
    });

    const unlistenCompleted = listen<{
      file_count: number;
      task_id: string;
    }>("large-batch-completed", (event) => {
      setProgress({
        active: false,
        processed: event.payload.file_count,
        total: event.payload.file_count,
        taskId: event.payload.task_id,
        phase: "writing",
      });
    });

    return () => {
      unlistenStarted.then((fn) => fn());
      unlistenProgress.then((fn) => fn());
      unlistenCompleted.then((fn) => fn());
    };
  }, []);

  return progress;
}
