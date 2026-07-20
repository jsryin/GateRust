import { LoaderCircle, LogOut, RefreshCw, Save, ShieldCheck } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import type { ClientStatus, ConfigResponse } from './lib/client-types';
import { desktop } from './lib/desktop';
import { ConnectionSettings } from './components/ConnectionSettings';
import { ServiceList } from './components/ServiceList';
import { StatusPane } from './components/StatusPane';
import { createEditableConfig, prepareConfig } from './lib/config';
import type { EditableClientConfig, EditableService, Notice, NoticeKind } from './lib/types';

const startingStatus: ClientStatus = {
  state: 'starting',
  message: null,
  server: null,
  device_id: null,
  retry_seconds: null
};

const offlineStatus: ClientStatus = {
  state: 'offline',
  message: '客户端运行时不可用',
  server: null,
  device_id: null,
  retry_seconds: null
};

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error) return error.message;
  return typeof error === 'string' ? error : fallback;
}

export function App() {
  const [baseline, setBaseline] = useState('');
  const [configPath, setConfigPath] = useState('');
  const [draft, setDraft] = useState<EditableClientConfig | null>(null);
  const [fatalError, setFatalError] = useState('');
  const [generating, setGenerating] = useState(false);
  const [loading, setLoading] = useState(true);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<ClientStatus>(startingStatus);
  const [version, setVersion] = useState('');
  const dirtyRef = useRef(false);
  const noticeTimer = useRef<number | undefined>(undefined);

  const preparedConfig = useMemo(() => (draft ? prepareConfig(draft) : null), [draft]);
  const dirty = Boolean(
    preparedConfig && baseline && JSON.stringify(preparedConfig) !== baseline
  );
  const pollingEnabled = Boolean(draft && !fatalError);

  const applyConfig = useCallback((response: ConfigResponse) => {
    const editable = createEditableConfig(response.config);
    setBaseline(editable.baseline);
    setConfigPath(response.path);
    setDraft(editable.draft);
  }, []);

  const showNotice = useCallback((message: string, kind: NoticeKind) => {
    window.clearTimeout(noticeTimer.current);
    setNotice({ kind, message });
    noticeTimer.current = window.setTimeout(() => setNotice(null), 4_000);
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await desktop.getStatus());
    } catch {
      setStatus(offlineStatus);
    }
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setFatalError('');
    try {
      const [config, currentStatus, version] = await Promise.all([
        desktop.getConfig(),
        desktop.getStatus(),
        desktop.getVersion()
      ]);
      applyConfig(config);
      setStatus(currentStatus);
      setVersion(version);
    } catch (error) {
      setStatus(offlineStatus);
      setFatalError(errorMessage(error, '客户端配置加载失败'));
    } finally {
      setLoading(false);
    }
  }, [applyConfig]);

  useEffect(() => {
    void load();
    return () => {
      window.clearTimeout(noticeTimer.current);
    };
  }, [load]);

  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);

  useEffect(() => {
    let disposed = false;
    let removeCloseListener: (() => void) | undefined;
    void desktop
      .onCloseRequested(() => dirtyRef.current)
      .then((remove) => {
        if (disposed) {
          remove();
        } else {
          removeCloseListener = remove;
        }
      })
      .catch((error: unknown) => {
        showNotice(errorMessage(error, '无法监听窗口关闭事件'), 'error');
      });
    return () => {
      disposed = true;
      removeCloseListener?.();
    };
  }, [showNotice]);

  useEffect(() => {
    if (!pollingEnabled) return;

    let disposed = false;
    let statusTimer: number | undefined;

    // 递归定时器在上一次 IPC 完成后再调度，避免后台阻塞时堆积请求。
    const schedule = () => {
      window.clearTimeout(statusTimer);
      if (disposed || document.hidden) return;
      statusTimer = window.setTimeout(() => {
        void refreshStatus().finally(schedule);
      }, 2_000);
    };

    const handleVisibilityChange = () => {
      window.clearTimeout(statusTimer);
      if (!document.hidden) {
        void refreshStatus().finally(schedule);
      }
    };

    schedule();
    document.addEventListener('visibilitychange', handleVisibilityChange);
    return () => {
      disposed = true;
      window.clearTimeout(statusTimer);
      document.removeEventListener('visibilitychange', handleVisibilityChange);
    };
  }, [pollingEnabled, refreshStatus]);

  const updateGroupKey = useCallback((key: string) => {
    setDraft((current) => (current ? { ...current, key } : current));
  }, []);

  const updateServer = useCallback((server: EditableClientConfig['server']) => {
    setDraft((current) => (current ? { ...current, server } : current));
  }, []);

  const updateServices = useCallback(
    (update: (services: EditableService[]) => EditableService[]) => {
      setDraft((current) =>
        current ? { ...current, services: update(current.services) } : current
      );
    },
    []
  );

  async function save(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (!preparedConfig || saving) return;

    setSaving(true);
    try {
      applyConfig(await desktop.saveConfig(preparedConfig));
      showNotice('配置已保存，连接正在更新', 'success');
      await refreshStatus();
    } catch (error) {
      showNotice(errorMessage(error, '保存配置失败'), 'error');
    } finally {
      setSaving(false);
    }
  }

  async function generateKey(): Promise<void> {
    if (generating) return;
    setGenerating(true);
    try {
      updateGroupKey(await desktop.generateKey());
    } catch (error) {
      showNotice(errorMessage(error, '生成密钥失败'), 'error');
    } finally {
      setGenerating(false);
    }
  }

  async function chooseCertificate(): Promise<void> {
    try {
      const path = await desktop.chooseCertificate();
      if (path) {
        setDraft((current) =>
          current
            ? { ...current, server: { ...current.server, caCertificate: path } }
            : current
        );
      }
    } catch (error) {
      showNotice(errorMessage(error, '选择证书失败'), 'error');
    }
  }

  async function quit(): Promise<void> {
    try {
      await desktop.quit(dirty);
    } catch (error) {
      showNotice(errorMessage(error, '退出客户端失败'), 'error');
    }
  }

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark">
            <ShieldCheck aria-hidden="true" size={21} strokeWidth={2} />
          </span>
          <span className="brand-name">
            <strong>GateRust</strong>
            <small>{version ? `Client v${version}` : 'Client'}</small>
          </span>
        </div>

        <StatusPane status={status} />

        <div className="header-actions">
          <button
            aria-label="退出客户端"
            className="icon-button quit-button"
            onClick={() => void quit()}
            title="退出客户端"
            type="button"
          >
            <LogOut aria-hidden="true" size={18} strokeWidth={1.8} />
          </button>
          <button
            className="primary-button"
            disabled={!dirty || saving || loading}
            form="config-form"
            type="submit"
          >
            {saving ? (
              <LoaderCircle aria-hidden="true" className="spin" size={16} />
            ) : (
              <Save aria-hidden="true" size={16} strokeWidth={2} />
            )}
            {saving ? '保存中' : '保存配置'}
          </button>
        </div>
      </header>

      <main className="workspace">
        <div className="workspace-inner">
          <header className="page-heading">
            <div>
              <h1>连接配置</h1>
              <p title={configPath}>{configPath || '正在读取配置路径'}</p>
            </div>
          </header>

          {loading ? (
            <div className="loading-state">
              <LoaderCircle aria-hidden="true" className="spin" size={22} />
              <span>正在启动客户端</span>
            </div>
          ) : fatalError ? (
            <div className="fatal-state" role="alert">
              <strong>无法加载客户端</strong>
              <p>{fatalError}</p>
              <button className="secondary-button" onClick={() => void load()} type="button">
                <RefreshCw aria-hidden="true" size={16} />
                重试
              </button>
            </div>
          ) : draft ? (
            <form id="config-form" onSubmit={(event) => void save(event)}>
              <fieldset className="settings-surface" disabled={saving}>
                <ConnectionSettings
                  generating={generating}
                  groupKey={draft.key}
                  onChooseCertificate={chooseCertificate}
                  onGenerateKey={generateKey}
                  onGroupKeyChange={updateGroupKey}
                  onServerChange={updateServer}
                  server={draft.server}
                />
                <ServiceList onChange={updateServices} services={draft.services} />
              </fieldset>
            </form>
          ) : null}
        </div>
      </main>

      {notice && (
        <div className={`toast ${notice.kind}`} role={notice.kind === 'error' ? 'alert' : 'status'}>
          {notice.message}
        </div>
      )}
    </div>
  );
}
