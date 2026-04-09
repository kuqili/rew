import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { listTasks, getTaskChanges, getStatus, type TaskInfo, type ChangeInfo, type StatusInfo } from "../lib/tauri";

export function useTasks() {
  const [tasks, setTasks] = useState<TaskInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const data = await listTasks();
      setTasks(data || []);
      setError(null);
    } catch (e) {
      console.error("[useTasks] FAILED:", e);
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();

    // Poll every 5 seconds
    const timer = setInterval(refresh, 5000);

    // Also refresh on snapshot/task events
    const unlistenSnap = listen("snapshot-created", () => refresh());
    const unlistenTask = listen("task-updated", () => refresh());

    return () => {
      clearInterval(timer);
      unlistenSnap.then(fn => fn());
      unlistenTask.then(fn => fn());
    };
  }, [refresh]);

  return { tasks, loading, error, refresh };
}

export function useTaskChanges(taskId: string | null) {
  const [changes, setChanges] = useState<ChangeInfo[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!taskId) {
      setChanges([]);
      return;
    }

    setLoading(true);
    getTaskChanges(taskId)
      .then(setChanges)
      .catch(() => setChanges([]))
      .finally(() => setLoading(false));
  }, [taskId]);

  return { changes, loading };
}

export function useStatus() {
  const [status, setStatus] = useState<StatusInfo | null>(null);

  useEffect(() => {
    const refresh = async () => {
      try {
        setStatus(await getStatus());
      } catch {}
    };

    refresh();
    const timer = setInterval(refresh, 5000);
    return () => clearInterval(timer);
  }, []);

  return status;
}
