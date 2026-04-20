import { useState, useCallback } from 'react'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

export type UpdateStatus =
  | 'idle'
  | 'checking'
  | 'available'
  | 'downloading'
  | 'ready'
  | 'up-to-date'
  | 'error'

export interface UpdateInfo {
  version: string
  body: string | null
}

export function useUpdater() {
  const [status, setStatus] = useState<UpdateStatus>('idle')
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null)
  const [progress, setProgress] = useState(0)         // 0-100
  const [error, setError] = useState<string | null>(null)

  const checkForUpdates = useCallback(async () => {
    setStatus('checking')
    setError(null)

    try {
      const update = await check()

      if (!update) {
        setStatus('up-to-date')
        return
      }

      setUpdateInfo({ version: update.version, body: update.body ?? null })
      setStatus('available')
    } catch (e) {
      setError(String(e))
      setStatus('error')
    }
  }, [])

  const downloadAndInstall = useCallback(async () => {
    if (status !== 'available' || !updateInfo) return

    setStatus('downloading')
    setProgress(0)

    try {
      const update = await check()
      if (!update) return

      let downloaded = 0
      let total = 0

      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case 'Started':
            total = event.data.contentLength ?? 0
            break
          case 'Progress':
            downloaded += event.data.chunkLength
            if (total > 0) {
              setProgress(Math.round((downloaded / total) * 100))
            }
            break
          case 'Finished':
            setProgress(100)
            setStatus('ready')
            break
        }
      })
    } catch (e) {
      setError(String(e))
      setStatus('error')
    }
  }, [status, updateInfo])

  const restart = useCallback(async () => {
    await relaunch()
  }, [])

  return {
    status,
    updateInfo,
    progress,
    error,
    checkForUpdates,
    downloadAndInstall,
    restart,
  }
}
