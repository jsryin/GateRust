import { access } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { app } from 'electron';
import type { ClientConfig, ClientStatus, ConfigResponse } from '../shared/types';

const API_ORIGIN = 'http://127.0.0.1:47823';
const START_ATTEMPTS = 40;
const START_INTERVAL_MS = 250;
const REQUEST_TIMEOUT_MS = 5_000;

interface SessionResponse {
  token: string;
}

interface KeyResponse {
  key: string;
}

interface ErrorResponse {
  error?: string;
}

export class BackendClient {
  private child?: ChildProcess;
  private token?: string;
  private authentication?: Promise<string>;
  private stopping = false;

  async start(configPath?: string): Promise<void> {
    if (this.stopping) {
      throw new Error('客户端正在退出');
    }
    if (await this.isHealthy()) {
      await this.authenticate();
      return;
    }

    const previous = this.child;
    if (previous && this.isRunning(previous)) {
      previous.kill();
      await Promise.race([once(previous, 'exit'), delay(500)]);
    }

    const binary = await this.resolveBinary();
    if (this.stopping) {
      throw new Error('客户端正在退出');
    }
    const args = configPath ? ['--config', configPath] : [];
    const child = spawn(binary, args, {
      stdio: app.isPackaged ? 'ignore' : 'inherit',
      windowsHide: true
    });
    this.child = child;
    let spawnError: Error | undefined;
    child.once('error', (error) => {
      spawnError = error;
    });

    for (let attempt = 0; attempt < START_ATTEMPTS; attempt += 1) {
      if (spawnError) {
        throw new Error(`无法启动客户端后台：${spawnError.message}`);
      }
      if (!this.isRunning(child)) {
        throw new Error(`客户端后台已退出（${child.exitCode ?? child.signalCode ?? 'unknown'}）`);
      }
      if (await this.isHealthy()) {
        await this.authenticate();
        return;
      }
      await delay(START_INTERVAL_MS);
    }
    child.kill();
    throw new Error('客户端后台启动超时');
  }

  getConfig(): Promise<ConfigResponse> {
    return this.request('/api/config');
  }

  saveConfig(config: ClientConfig): Promise<ConfigResponse> {
    return this.request('/api/config', {
      method: 'PUT',
      body: JSON.stringify(config)
    });
  }

  getStatus(): Promise<ClientStatus> {
    return this.request('/api/status');
  }

  async generateKey(): Promise<string> {
    const response = await this.request<KeyResponse>('/api/key', { method: 'POST' });
    return response.key;
  }

  async stop(): Promise<void> {
    this.stopping = true;
    const child = this.child;
    if (await this.isHealthy()) {
      try {
        await this.request<void>('/api/shutdown', { method: 'POST' });
      } catch (error) {
        console.warn('请求客户端后台退出失败', error);
      }
    }
    if (!child || !this.isRunning(child)) {
      return;
    }
    await Promise.race([once(child, 'exit'), delay(2_500)]);
    if (this.isRunning(child)) {
      child.kill();
      await Promise.race([once(child, 'exit'), delay(500)]);
    }
  }

  private async request<T>(path: string, init: RequestInit = {}, retry = true): Promise<T> {
    const token = await this.authenticate();
    const response = await fetch(`${API_ORIGIN}${path}`, {
      ...init,
      headers: {
        Authorization: `Bearer ${token}`,
        ...(init.body ? { 'Content-Type': 'application/json' } : {}),
        ...init.headers
      },
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS)
    });
    if (response.status === 401 && retry) {
      this.token = undefined;
      return this.request(path, init, false);
    }
    if (!response.ok) {
      const body = (await response.json().catch(() => ({}))) as ErrorResponse;
      throw new Error(body.error ?? `客户端后台请求失败（${response.status}）`);
    }
    return (response.status === 204 ? undefined : await response.json()) as T;
  }

  private async authenticate(): Promise<string> {
    if (this.token) {
      return this.token;
    }
    if (!this.authentication) {
      this.authentication = this.createSession().finally(() => {
        this.authentication = undefined;
      });
    }
    this.token = await this.authentication;
    return this.token;
  }

  private async createSession(): Promise<string> {
    const response = await fetch(`${API_ORIGIN}/api/session`, {
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS)
    });
    if (!response.ok) {
      throw new Error(`无法建立本机会话（${response.status}）`);
    }
    const session = (await response.json()) as SessionResponse;
    return session.token;
  }

  private async isHealthy(): Promise<boolean> {
    try {
      const response = await fetch(`${API_ORIGIN}/api/health`, {
        signal: AbortSignal.timeout(750)
      });
      return response.ok && response.headers.get('x-gaterust-client') === '1';
    } catch {
      return false;
    }
  }

  private async resolveBinary(): Promise<string> {
    const override = process.env.GATERUST_CLIENT_BINARY;
    const binary = override
      ? resolve(override)
      : app.isPackaged
        ? join(process.resourcesPath, 'bin', this.binaryName())
        : resolve(app.getAppPath(), '..', 'target', 'debug', this.binaryName());
    try {
      await access(binary);
    } catch {
      throw new Error(`未找到客户端后台程序：${binary}`);
    }
    return binary;
  }

  private binaryName(): string {
    return process.platform === 'win32' ? 'gaterust-client.exe' : 'gaterust-client';
  }

  private isRunning(child: ChildProcess): boolean {
    return child.exitCode === null && child.signalCode === null;
  }
}
