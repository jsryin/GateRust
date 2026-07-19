import { contextBridge, ipcRenderer } from 'electron';
import { channels } from '../shared/channels';
import type {
  AppInfo,
  ClientConfig,
  ClientStatus,
  ConfigResponse,
  DesktopBridge,
  DesktopResult
} from '../shared/types';

async function invoke<T>(channel: string, ...args: unknown[]): Promise<T> {
  const response = (await ipcRenderer.invoke(channel, ...args)) as DesktopResult<T>;
  if (!response.ok) {
    throw new Error(response.error);
  }
  return response.value;
}

const bridge: DesktopBridge = {
  getAppInfo: () => invoke<AppInfo>(channels.appInfo),
  getConfig: () => invoke<ConfigResponse>(channels.configGet),
  saveConfig: (config: ClientConfig) => invoke<ConfigResponse>(channels.configSave, config),
  getStatus: () => invoke<ClientStatus>(channels.status),
  generateKey: () => invoke<string>(channels.keyGenerate),
  chooseCertificate: () => invoke<string | null>(channels.chooseCertificate),
  setDirty: (dirty: boolean) => ipcRenderer.send(channels.dirty, dirty),
  quit: () => invoke<void>(channels.quit)
};

contextBridge.exposeInMainWorld('gaterust', bridge);
