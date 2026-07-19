import type {
  CertificateConfig,
  ClientService,
  ConfigSnapshot,
  Dashboard,
  GroupConfig,
  ProxyConfig,
  ProxyListenerConfig,
  RouteConfig,
  ServerConfig,
  ServerQuicConfig,
  TunnelConfig,
  TunnelRuntimeState
} from './types';

const API_BASE = (import.meta.env.VITE_API_BASE as string | undefined)?.replace(/\/$/, '') ?? '';

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
  }
}

async function request<T>(path: string, token: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    ...init,
    headers: {
      ...(init.body ? { 'Content-Type': 'application/json' } : {}),
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...init.headers
    }
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: `请求失败 (${response.status})` }));
    throw new ApiError(response.status, body.error ?? `请求失败 (${response.status})`);
  }
  return response.status === 204 ? (undefined as T) : response.json();
}

export async function login(username: string, password: string) {
  return request<{ token: string; expires_at: number }>('/api/auth/login', '', {
    method: 'POST',
    body: JSON.stringify({ username, password })
  });
}

export const getConfig = (token: string) => request<ConfigSnapshot>('/api/config', token);
export const checkSession = (token: string) => request<void>('/api/auth/session', token);

function namedPath(base: string, name: string) {
  return `${base}/${encodeURIComponent(name)}`;
}

function writeConfig<TConfig, TValue>(path: string, token: string, method: 'POST' | 'PUT', value: TValue) {
  return request<TConfig>(path, token, {
    method,
    body: JSON.stringify(value)
  });
}

const deleteConfig = <TConfig>(path: string, token: string) =>
  request<TConfig>(path, token, { method: 'DELETE' });

export const setTunnelQuic = (token: string, quic: ServerQuicConfig) =>
  writeConfig<ServerConfig, ServerQuicConfig>('/api/config/tunnel/quic', token, 'PUT', quic);

export const createGroup = (token: string, group: GroupConfig) =>
  writeConfig<ServerConfig, GroupConfig>('/api/config/tunnel/groups', token, 'POST', group);

export const updateGroup = (token: string, name: string, group: GroupConfig) =>
  writeConfig<ServerConfig, GroupConfig>(namedPath('/api/config/tunnel/groups', name), token, 'PUT', group);

export const deleteGroup = (token: string, name: string) =>
  deleteConfig<ServerConfig>(namedPath('/api/config/tunnel/groups', name), token);

export const createTunnel = (token: string, tunnel: TunnelConfig) =>
  writeConfig<ServerConfig, TunnelConfig>('/api/config/tunnel/tunnels', token, 'POST', tunnel);

export const updateTunnel = (token: string, name: string, tunnel: TunnelConfig) =>
  writeConfig<ServerConfig, TunnelConfig>(namedPath('/api/config/tunnel/tunnels', name), token, 'PUT', tunnel);

export const deleteTunnel = (token: string, name: string) =>
  deleteConfig<ServerConfig>(namedPath('/api/config/tunnel/tunnels', name), token);

export const setProxyListener = (token: string, listener: ProxyListenerConfig) =>
  writeConfig<ProxyConfig, ProxyListenerConfig>('/api/config/proxy/listener', token, 'PUT', listener);

export const createCertificate = (token: string, certificate: CertificateConfig) =>
  writeConfig<ProxyConfig, CertificateConfig>('/api/config/proxy/certificates', token, 'POST', certificate);

export const updateCertificate = (token: string, name: string, certificate: CertificateConfig) =>
  writeConfig<ProxyConfig, CertificateConfig>(namedPath('/api/config/proxy/certificates', name), token, 'PUT', certificate);

export const deleteCertificate = (token: string, name: string) =>
  deleteConfig<ProxyConfig>(namedPath('/api/config/proxy/certificates', name), token);

export const createRoute = (token: string, route: RouteConfig) =>
  writeConfig<ProxyConfig, RouteConfig>('/api/config/proxy/routes', token, 'POST', route);

export const updateRoute = (token: string, name: string, route: RouteConfig) =>
  writeConfig<ProxyConfig, RouteConfig>(namedPath('/api/config/proxy/routes', name), token, 'PUT', route);

export const deleteRoute = (token: string, name: string) =>
  deleteConfig<ProxyConfig>(namedPath('/api/config/proxy/routes', name), token);

export const generateKey = (token: string) =>
  request<{ key: string }>('/api/groups/key', token, { method: 'POST' });

export const getTunnelRuntime = (token: string, signal?: AbortSignal) =>
  request<TunnelRuntimeState>('/api/tunnel/runtime', token, { signal });

export const disconnectTunnelClient = (token: string, sessionId: number) =>
  request<void>(`/api/tunnel/sessions/${sessionId}`, token, { method: 'DELETE' });

export function generateClientConfig(
  token: string,
  payload: {
    group: string;
    server_address: string;
    server_name: string | null;
    ca_certificate: string | null;
    services: ClientService[];
  }
) {
  return request<{ toml: string }>('/api/client-config', token, {
    method: 'POST',
    body: JSON.stringify(payload)
  });
}

export async function streamDashboard(
  token: string,
  signal: AbortSignal,
  onDashboard: (dashboard: Dashboard) => void
) {
  const response = await fetch(`${API_BASE}/api/events`, {
    headers: { Authorization: `Bearer ${token}` },
    signal
  });
  if (!response.ok || !response.body) throw new ApiError(response.status, '实时状态连接失败');
  const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
  let buffer = '';
  while (true) {
    const { value, done } = await reader.read();
    if (done) return;
    buffer += value;
    const events = buffer.split('\n\n');
    buffer = events.pop() ?? '';
    for (const event of events) {
      const data = event.split('\n').find((line) => line.startsWith('data:'))?.slice(5).trim();
      if (data) onDashboard(JSON.parse(data) as Dashboard);
    }
  }
}
