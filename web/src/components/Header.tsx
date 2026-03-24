import { ReactNode } from 'react';
import { ConnectionState } from '../types';

interface HeaderProps {
  logo: ReactNode;
  connectionState: ConnectionState;
  version: string;
  onSettingsClick: () => void;
}

export function Header({ logo, connectionState, version, onSettingsClick }: HeaderProps) {
  const statusText = {
    [ConnectionState.Connecting]: 'Connecting...',
    [ConnectionState.Connected]: 'Connected',
    [ConnectionState.Disconnected]: 'Disconnected',
    [ConnectionState.Reconnecting]: 'Reconnecting...',
    [ConnectionState.Error]: 'Error',
  };

  const isDisconnected = connectionState === ConnectionState.Disconnected ||
                         connectionState === ConnectionState.Error;

  return (
    <div className="header">
      <h1>{logo} Manta AI Terminal</h1>
      <div className="header-center">
        <span className="version">{version}</span>
        <div className="status">
          <span className={`status-dot ${isDisconnected ? 'disconnected' : ''}`}></span>
          <span>{statusText[connectionState]}</span>
        </div>
      </div>
      <button className="settings-btn" onClick={onSettingsClick} title="Settings">
        ⚙️
      </button>
    </div>
  );
}
