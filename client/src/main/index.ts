import { join } from 'node:path';
import { app, BrowserWindow, dialog, ipcMain } from 'electron';
import type { MessageBoxOptions, OpenDialogOptions } from 'electron';
import { BackendClient } from './backend';
import { channels } from '../shared/channels';
import type { ClientConfig, DesktopResult } from '../shared/types';

const backend = new BackendClient();
const configPath = configPathFromArguments();
let backendReady: Promise<void> | undefined;
let mainWindow: BrowserWindow | null = null;
let hasUnsavedChanges = false;
let quitAllowed = false;
let quitInProgress = false;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : '发生未知错误';
}

async function result<T>(operation: () => Promise<T> | T): Promise<DesktopResult<T>> {
  try {
    return { ok: true, value: await operation() };
  } catch (error) {
    return { ok: false, error: errorMessage(error) };
  }
}

function configPathFromArguments(): string | undefined {
  if (process.env.GATERUST_CLIENT_CONFIG) {
    return process.env.GATERUST_CLIENT_CONFIG;
  }
  const index = process.argv.findIndex((argument) => argument === '--config' || argument === '-c');
  const value = index >= 0 ? process.argv[index + 1] : undefined;
  return value && !value.startsWith('-') ? value : undefined;
}

function ensureBackend(): Promise<void> {
  if (!backendReady) {
    backendReady = backend.start(configPath).catch((error: unknown) => {
      backendReady = undefined;
      throw error;
    });
  }
  return backendReady;
}

function registerIpc(): void {
  ipcMain.handle(channels.appInfo, () => result(() => ({ version: app.getVersion() })));
  ipcMain.handle(channels.configGet, () =>
    result(async () => {
      await ensureBackend();
      return backend.getConfig();
    })
  );
  ipcMain.handle(channels.configSave, (_event, config: ClientConfig) =>
    result(async () => {
      await ensureBackend();
      return backend.saveConfig(config);
    })
  );
  ipcMain.handle(channels.status, () =>
    result(async () => {
      await ensureBackend();
      return backend.getStatus();
    })
  );
  ipcMain.handle(channels.keyGenerate, () =>
    result(async () => {
      await ensureBackend();
      return backend.generateKey();
    })
  );
  ipcMain.handle(channels.chooseCertificate, () =>
    result(async () => {
      const options: OpenDialogOptions = {
        title: '选择 CA 证书',
        properties: ['openFile'],
        filters: [
          { name: '证书文件', extensions: ['pem', 'crt', 'cer'] },
          { name: '所有文件', extensions: ['*'] }
        ]
      };
      const response = mainWindow
        ? await dialog.showOpenDialog(mainWindow, options)
        : await dialog.showOpenDialog(options);
      return response.canceled ? null : (response.filePaths[0] ?? null);
    })
  );
  ipcMain.on(channels.dirty, (_event, dirty: boolean) => {
    hasUnsavedChanges = dirty;
  });
  ipcMain.handle(channels.quit, () =>
    result(async () => {
      await requestQuit();
    })
  );
}

function createWindow(): void {
  mainWindow = new BrowserWindow({
    width: 1_120,
    height: 760,
    minWidth: 820,
    minHeight: 620,
    show: false,
    backgroundColor: '#f3f5f4',
    autoHideMenuBar: true,
    title: 'GateRust Client',
    webPreferences: {
      preload: join(__dirname, '../preload/index.cjs'),
      contextIsolation: true,
      devTools: !app.isPackaged,
      nodeIntegration: false,
      sandbox: true
    }
  });

  mainWindow.once('ready-to-show', () => mainWindow?.show());
  mainWindow.on('close', (event) => {
    if (!quitAllowed) {
      event.preventDefault();
      void requestQuit();
    }
  });
  mainWindow.on('closed', () => {
    mainWindow = null;
  });
  mainWindow.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  mainWindow.webContents.on('will-navigate', (event) => event.preventDefault());
  mainWindow.webContents.session.setPermissionCheckHandler(() => false);
  mainWindow.webContents.session.setPermissionRequestHandler((_webContents, _permission, callback) => {
    callback(false);
  });

  const rendererUrl = process.env.ELECTRON_RENDERER_URL;
  if (rendererUrl) {
    void mainWindow.loadURL(rendererUrl);
  } else {
    void mainWindow.loadFile(join(__dirname, '../renderer/index.html'));
  }
}

async function requestQuit(): Promise<void> {
  if (quitInProgress) {
    return;
  }
  quitInProgress = true;
  if (hasUnsavedChanges) {
    const options: MessageBoxOptions = {
      type: 'warning',
      title: '退出 GateRust Client',
      message: '当前配置尚未保存',
      detail: '退出后，本次修改将不会保留。',
      buttons: ['放弃修改并退出', '继续编辑'],
      defaultId: 1,
      cancelId: 1,
      noLink: true
    };
    const response = mainWindow
      ? await dialog.showMessageBox(mainWindow, options)
      : await dialog.showMessageBox(options);
    if (response.response !== 0) {
      quitInProgress = false;
      return;
    }
  }

  await backend.stop();
  quitAllowed = true;
  app.quit();
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (!mainWindow) {
      return;
    }
    if (mainWindow.isMinimized()) {
      mainWindow.restore();
    }
    mainWindow.show();
    mainWindow.focus();
  });

  app.on('before-quit', (event) => {
    if (!quitAllowed) {
      event.preventDefault();
      void requestQuit();
    }
  });

  void app.whenReady().then(() => {
    registerIpc();
    void ensureBackend().catch(() => undefined);
    createWindow();
  });
}
