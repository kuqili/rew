import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { listTasks, getTaskChanges, getStatus, type TaskInfo, type ChangeInfo, type DeletedDirSummaryInfo, type StatusInfo } from "../lib/tauri";

export function useTasks(dirFilter?: string | null, options?: { enabled?: boolean }) {
  const [tasks, setTasks] = useState<TaskInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const enabled = options?.enabled ?? true;

  const refresh = useCallback(async () => {
    if (!enabled) {
      setTasks([]);
      setError(null);
      setLoading(false);
      return;
    }
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
  }, [dirFilter, enabled]);

  useEffect(() => {
    if (!enabled) {
      setTasks([]);
      setError(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    refresh();

    const timer = setInterval(refresh, 5000);

    const unlistenTask = listen("task-updated", () => refresh());

    return () => {
      clearInterval(timer);
      unlistenTask.then(fn => fn());
    };
  }, [refresh, enabled]);

  return { tasks, loading, error, refresh };
}

export function useTaskChanges(taskId: string | null, dirFilter?: string | null) {
  const [changes, setChanges] = useState<ChangeInfo[]>([]);
  const [deletedDirs, setDeletedDirs] = useState<DeletedDirSummaryInfo[]>([]);
  const [totalCount, setTotalCount] = useState(0);
  const [truncated, setTruncated] = useState(false);
  const [initialLoading, setInitialLoading] = useState(true);

  const refresh = useCallback(async () => {
    if (!taskId) { setChanges([]); setDeletedDirs([]); setTotalCount(0); setTruncated(false); return; }
    getTaskChanges(taskId, dirFilter ?? undefined)
      .then((res) => {
        setChanges(res.changes);
        setDeletedDirs(res.deleted_dirs);
        setTotalCount(res.total_count);
        setTruncated(res.truncated);
      })
      .catch(() => {});
  }, [taskId, dirFilter]);

  useEffect(() => {
    setInitialLoading(true);
    if (!taskId) { setChanges([]); setDeletedDirs([]); setTotalCount(0); setTruncated(false); setInitialLoading(false); return; }
    getTaskChanges(taskId, dirFilter ?? undefined)
      .then((res) => {
        setChanges(res.changes);
        setDeletedDirs(res.deleted_dirs);
        setTotalCount(res.total_count);
        setTruncated(res.truncated);
      })
      .catch(() => { setChanges([]); setDeletedDirs([]); setTotalCount(0); setTruncated(false); })
      .finally(() => setInitialLoading(false));
  }, [taskId, dirFilter]);

  return { changes, deletedDirs, totalCount, truncated, loading: initialLoading, refresh };
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
