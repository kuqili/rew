import { useUpdater } from '../hooks/useUpdater'

export function UpdaterDialog() {
  const {
    status,
    updateInfo,
    progress,
    error,
    checkForUpdates,
    downloadAndInstall,
    restart,
  } = useUpdater()

  return (
    <div style={{
      padding: '16px',
      display: 'flex',
      flexDirection: 'column',
      gap: '12px',
      minWidth: '260px',
    }}>
      {/* 标题行 */}
      <div style={{ fontSize: '13px', fontWeight: 600, color: '#111' }}>
        软件更新
      </div>

      {/* 状态区 */}
      {status === 'idle' && (
        <button onClick={checkForUpdates} style={btnStyle}>
          检查更新
        </button>
      )}

      {status === 'checking' && (
        <p style={descStyle}>正在检查更新…</p>
      )}

      {status === 'up-to-date' && (
        <p style={descStyle}>✅ 已是最新版本</p>
      )}

      {status === 'available' && updateInfo && (
        <>
          <p style={descStyle}>
            发现新版本 <strong>{updateInfo.version}</strong>
          </p>
          {updateInfo.body && (
            <p style={{ ...descStyle, fontSize: '11px', color: '#888', whiteSpace: 'pre-wrap' }}>
              {updateInfo.body}
            </p>
          )}
          <button onClick={downloadAndInstall} style={btnStyle}>
            下载并安装
          </button>
        </>
      )}

      {status === 'downloading' && (
        <>
          <p style={descStyle}>正在下载… {progress}%</p>
          <div style={{
            height: '4px',
            borderRadius: '2px',
            background: '#e5e5e5',
            overflow: 'hidden',
          }}>
            <div style={{
              height: '100%',
              width: `${progress}%`,
              background: '#111',
              borderRadius: '2px',
              transition: 'width 0.2s ease',
            }} />
          </div>
        </>
      )}

      {status === 'ready' && (
        <>
          <p style={descStyle}>✅ 下载完成，重启后生效</p>
          <button onClick={restart} style={btnStyle}>
            立即重启
          </button>
        </>
      )}

      {status === 'error' && (
        <>
          <p style={{ ...descStyle, color: '#e53e3e' }}>
            更新失败：{error}
          </p>
          <button onClick={checkForUpdates} style={{ ...btnStyle, background: '#f5f5f5', color: '#333' }}>
            重试
          </button>
        </>
      )}
    </div>
  )
}

const btnStyle: React.CSSProperties = {
  padding: '7px 14px',
  borderRadius: '6px',
  border: 'none',
  background: '#111',
  color: '#fff',
  fontSize: '12px',
  fontWeight: 500,
  cursor: 'pointer',
  alignSelf: 'flex-start',
}

const descStyle: React.CSSProperties = {
  fontSize: '12px',
  color: '#555',
  margin: 0,
  lineHeight: 1.5,
}
