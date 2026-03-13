import { ReactNode } from 'react';
import { WebSocketState } from '../types';

interface HeaderProps {
  logo: ReactNode;
  wsState: WebSocketState;
  version: string;
  onSettingsClick: () => void;
}

export function Header({ logo, wsState, version, onSettingsClick }: HeaderProps) {
  const statusText = {
    [WebSocketState.Connecting]: 'Connecting...',
    [WebSocketState.Connected]: 'Connected',
    [WebSocketState.Disconnected]: 'Disconnected',
  };

  return (
    <div className="header">
      <h1>{logo} Manta AI Terminal</h1>
      <div className="header-center">
        <span className="version">{version}</span>
        <div className="status">
          <span className={`status-dot ${wsState === WebSocketState.Disconnected ? 'disconnected' : ''}`}></span>
          <span>{statusText[wsState]}</span>
        </div>
      </div>
      <button className="settings-btn" onClick={onSettingsClick} title="Settings">
        ⚙️
      </button>
    </div>
  );
}
