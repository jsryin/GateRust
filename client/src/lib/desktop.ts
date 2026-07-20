import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { confirm, open } from '@tauri-apps/plugin-dialog';
import type { ClientConfig, ClientStatus, ConfigResponse } from './client-types';

let quitting = false;

async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(name, args);
  } catch (error) {
    throw new Error(typeof error === 'string' ? error : '桌面客户端操作失败');
  }
}

async function quit(hasUnsavedChanges: boolean): Promise<void> {
  if (quitting) return;
  quitting = true;
  try {
    if (
      hasUnsavedChanges &&
      !(await confirm('退出后，本次修改将不会保留。', {
        title: '当前配置尚未保存',
        kind: 'warning',
        okLabel: '放弃修改并退出',
        cancelLabel: '继续编辑'
      }))
    ) {
      quitting = false;
      return;
    }
    await command<void>('shutdown');
    await getCurrentWindow().destroy();
  } catch (error) {
    quitting = false;
    throw error;
  }
}

export const desktop = {
  getVersion,
  getConfig: () => command<ConfigResponse>('get_config'),
  saveConfig: (config: ClientConfig) => command<ConfigResponse>('save_config', { config }),
  getStatus: () => command<ClientStatus>('get_status'),
  generateKey: () => command<string>('generate_key'),
  chooseCertificate: async () =>
    open({
      title: '选择 CA 证书',
      directory: false,
      multiple: false,
      filters: [{ name: '证书文件', extensions: ['pem', 'crt', 'cer'] }]
    }),
  onCloseRequested: (hasUnsavedChanges: () => boolean) =>
    getCurrentWindow().onCloseRequested((event) => {
      event.preventDefault();
      void quit(hasUnsavedChanges()).catch((error: unknown) => {
        console.error('关闭客户端失败', error);
      });
    }),
  quit,
  show: () => getCurrentWindow().show()
};
