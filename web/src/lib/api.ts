import type { ClientService, ConfigSnapshot, Dashboard, ProxyConfig, ServerConfig } from './types';

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

export const saveTunnel = (token: string, config: ServerConfig) =>
  request<ServerConfig>('/api/config/tunnel', token, {
    method: 'PUT',
    body: JSON.stringify(config)
  });

export const saveProxy = (token: string, config: ProxyConfig) =>
  request<ProxyConfig>('/api/config/proxy', token, {
    method: 'PUT',
    body: JSON.stringify(config)
  });

export const generateKey = (token: string) =>
  request<{ key: string }>('/api/groups/key', token, { method: 'POST' });

export function generateClientConfig(
  token: string,
  payload: {
    group: string;
    server_address: string;
    server_name: string;
    ca_certificate: string;
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
