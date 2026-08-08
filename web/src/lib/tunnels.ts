import type { TunnelConfig } from './types';

export function tunnelLocalTarget(tunnel: TunnelConfig): string | null {
  if (tunnel.kind === 'socks5') return null;
  const separator = tunnel.bind.lastIndexOf(':');
  const port = tunnel.local_port ?? (separator >= 0 ? tunnel.bind.slice(separator + 1) : '8080');
  const host = tunnel.local_ip.includes(':') ? `[${tunnel.local_ip}]` : tunnel.local_ip;
  return `${host}:${port}`;
}
