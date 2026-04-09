import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { listTasks, getTaskChanges, getStatus, type TaskInfo, type ChangeInfo, type StatusInfo } from "../lib/tauri";

export function useTasks(dirFilter?: string | null) {
  const [tasks, setTasks] = useState<TaskInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const data = await listTasks(dirFilter ?? undefined);
      setTasks(data || []);
      setError(null);
    } catch (e) {
      console.error("[useTasks] FAILED:", e);
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [dirFilter]);

  useEffect(() => {
    setLoading(true);
    refresh();

    const timer = setInterval(refresh, 5000);

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

export function useTaskChanges(taskId: string | null, dirFilter?: string | null) {
  const [changes, setChanges] = useState<ChangeInfo[]>([]);
  const [initialLoading, setInitialLoading] = useState(true);

  const refresh = useCallback(async () => {
    if (!taskId) { setChanges([]); return; }
    getTaskChanges(taskId, dirFilter ?? undefined)
      .then(setChanges)
      .catch(() => {});
  }, [taskId, dirFilter]);

  useEffect(() => {
    setInitialLoading(true);
    if (!taskId) { setChanges([]); setInitialLoading(false); return; }
    getTaskChanges(taskId, dirFilter ?? undefined)
      .then(setChanges)
      .catch(() => setChanges([]))
      .finally(() => setInitialLoading(false));
  }, [taskId, dirFilter]);

  return { changes, loading: initialLoading, refresh };
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
