import { ConnectionState } from '../types';

interface HeaderProps {
  title?: string;
  connectionState: ConnectionState;
  version: string;
  onSettingsClick: () => void;
  onMenuClick: () => void;
  theme: 'light' | 'dark';
  onToggleTheme: () => void;
}

export function Header({
  title = 'AI 对话',
  connectionState,
  version,
  onSettingsClick,
  onMenuClick,
  theme,
  onToggleTheme,
}: HeaderProps) {
  const isDisconnected =
    connectionState === ConnectionState.Disconnected ||
    connectionState === ConnectionState.Error;

  return (
    <div className="header">
      <div className="header-left">
        <button className="menu-btn" onClick={onMenuClick} title="菜单">
          ☰
        </button>
        <span className="header-title">{title}</span>
      </div>

      <div className="header-right">
        <span className="version">{version}</span>
        <div className="status">
          <span className={`status-dot ${isDisconnected ? 'disconnected' : ''}`} />
          <span style={{ display: 'none' }}>{connectionState}</span>
        </div>
        <button className="theme-toggle" onClick={onToggleTheme} title="切换主题">
          {theme === 'dark' ? '☀' : '☾'}
        </button>
        <button className="theme-toggle" onClick={onSettingsClick} title="设置">
          ⚙
        </button>
      </div>
    </div>
  );
}
