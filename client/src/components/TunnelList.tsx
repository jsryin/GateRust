import { Cable, LockKeyhole } from 'lucide-react';
import type { ClientTunnel, TunnelKind } from '../lib/client-types';

interface TunnelListProps {
  onToggle: (name: string) => void;
  selected: Set<string>;
  tunnels: ClientTunnel[];
}

const kindLabels: Record<TunnelKind, string> = {
  tcp: 'TCP',
  udp: 'UDP',
  socks5: 'SOCKS5'
};

const stateLabels = {
  idle: '空闲',
  connected: '已连接',
  occupied: '被占用'
} as const;

export function TunnelList({ onToggle, selected, tunnels }: TunnelListProps) {
  if (!tunnels.length) {
    return <div className="empty-state"><Cable size={20} /><span>当前分组暂无隧道</span></div>;
  }

  return (
    <div className="tunnel-table">
      <div className="tunnel-table-head" aria-hidden="true">
        <span />
        <span>隧道</span>
        <span>服务器端口</span>
        <span>本地端口</span>
        <span>状态</span>
      </div>
      <div className="tunnel-rows">
        {tunnels.map((tunnel) => {
          const unavailable = tunnel.state !== 'idle';
          return (
            <label className={`tunnel-row ${tunnel.state}`} key={tunnel.name}>
              <input
                checked={tunnel.state === 'connected' || selected.has(tunnel.name)}
                disabled={unavailable}
                onChange={() => onToggle(tunnel.name)}
                type="checkbox"
              />
              <span className="tunnel-name">
                <strong>{tunnel.name}</strong>
                <small>{kindLabels[tunnel.kind]}</small>
              </span>
              <code>{tunnel.server_port}</code>
              <code>{tunnel.local_port ?? '-'}</code>
              <span className={`state-badge ${tunnel.state}`}>
                {tunnel.state === 'occupied' && <LockKeyhole size={12} />}
                {stateLabels[tunnel.state]}
              </span>
            </label>
          );
        })}
      </div>
    </div>
  );
}
