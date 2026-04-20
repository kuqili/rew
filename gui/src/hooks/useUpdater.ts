import { useState, useCallback, useRef } from 'react'
import { check, type Update } from '@tauri-apps/plugin-updater'
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

  // Cache the Update object from check() so downloadAndInstall doesn't
  // need to call check() a second time (which could return null on network
  // hiccups and silently abort the download with no user feedback).
  const updateRef = useRef<Update | null>(null)

  const checkForUpdates = useCallback(async () => {
    setStatus('checking')
    setError(null)
    updateRef.current = null

    try {
      const update = await check()

      if (!update) {
        setStatus('up-to-date')
        return
      }

      updateRef.current = update
      setUpdateInfo({ version: update.version, body: update.body ?? null })
      setStatus('available')
    } catch (e) {
      setError(String(e))
      setStatus('error')
    }
  }, [])

  const downloadAndInstall = useCallback(async () => {
    if (status !== 'available') return

    const update = updateRef.current
    if (!update) {
      setError('更新信息已丢失，请重新检查更新')
      setStatus('error')
      return
    }

    setStatus('downloading')
    setProgress(0)

    try {
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
  }, [status])

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
