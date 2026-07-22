import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { ClientConfig, ClientStatus } from './client-types';

let quitting = false;

async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(name, args);
  } catch (error) {
    throw new Error(typeof error === 'string' ? error : '桌面客户端操作失败');
  }
}

async function quit(): Promise<void> {
  if (quitting) return;
  quitting = true;
  try {
    await command<void>('shutdown');
    await getCurrentWindow().destroy();
  } catch (error) {
    quitting = false;
    throw error;
  }
}

export const desktop = {
  getVersion,
  getConfig: () => command<ClientConfig>('get_config'),
  getStatus: () => command<ClientStatus>('get_status'),
  login: (serverAddress: string, key: string) =>
    command<ClientConfig>('login', { serverAddress, key }),
  connectTunnels: (names: string[]) => command<void>('connect_tunnels', { names }),
  disconnectTunnels: () => command<void>('disconnect_tunnels'),
  quit,
  show: () => getCurrentWindow().show()
};
