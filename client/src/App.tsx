import { LoaderCircle, Pencil } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { LoginForm } from './components/LoginForm';
import { TunnelActions } from './components/TunnelActions';
import { TunnelList } from './components/TunnelList';
import type { ClientConfig, ClientStatus } from './lib/client-types';
import { desktop } from './lib/desktop';

const startingStatus: ClientStatus = {
  state: 'starting',
  message: null,
  server: null,
  device_id: null,
  retry_seconds: null,
  tunnels: []
};

const loginCancelledMessage = '已取消获取连接配置';

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error) return error.message;
  return typeof error === 'string' ? error : fallback;
}

export function App() {
  const [address, setAddress] = useState('');
  const [key, setKey] = useState('');
  const [status, setStatus] = useState<ClientStatus>(startingStatus);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [editingConfig, setEditingConfig] = useState(false);
  const [action, setAction] = useState<'enable' | 'disable' | null>(null);
  const [error, setError] = useState('');
  const onlineIdentity = useRef('');
  const knownTunnels = useRef<Set<string>>(new Set());
  const loginCancellationRequested = useRef(false);
  const configBeforeEdit = useRef<{ address: string; key: string } | null>(null);

  const applyConfig = useCallback((config: ClientConfig) => {
    setAddress(config.server.address);
    setKey(config.server.address ? config.key : '');
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await desktop.getStatus());
    } catch {
      setStatus((current) => ({
        ...current,
        state: 'offline',
        message: '客户端运行时不可用',
        tunnels: []
      }));
    }
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      const [config, currentStatus] = await Promise.all([
        desktop.getConfig(),
        desktop.getStatus()
      ]);
      applyConfig(config);
      setStatus(currentStatus);
    } catch (cause) {
      setError(errorMessage(cause, '客户端启动失败'));
    } finally {
      setLoading(false);
    }
  }, [applyConfig]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    const poll = async () => {
      await refreshStatus();
      if (!disposed && !document.hidden) timer = window.setTimeout(() => void poll(), 1_000);
    };
    const handleVisibility = () => {
      window.clearTimeout(timer);
      if (!document.hidden) void poll();
    };
    timer = window.setTimeout(() => void poll(), 1_000);
    document.addEventListener('visibilitychange', handleVisibility);
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', handleVisibility);
    };
  }, [refreshStatus]);

  useEffect(() => {
    if (status.state !== 'online' || !status.device_id) return;
    const identity = `${status.server ?? ''}/${status.device_id}`;
    const currentNames = new Set(status.tunnels.map((tunnel) => tunnel.name));
    setSelected((current) => {
      const sameIdentity = onlineIdentity.current === identity;
      const next = sameIdentity ? new Set(current) : new Set<string>();
      let changed = !sameIdentity && current.size > 0;
      for (const tunnel of status.tunnels) {
        if (
          tunnel.state === 'enabled' ||
          (tunnel.state === 'idle' && !knownTunnels.current.has(tunnel.name))
        ) {
          if (!next.has(tunnel.name)) {
            next.add(tunnel.name);
            changed = true;
          }
        }
        if (tunnel.state === 'occupied' && next.delete(tunnel.name)) {
          changed = true;
        }
      }
      for (const name of next) {
        if (!currentNames.has(name)) {
          next.delete(name);
          changed = true;
        }
      }
      return changed ? next : current;
    });
    onlineIdentity.current = identity;
    knownTunnels.current = currentNames;
  }, [status]);

  async function login(): Promise<void> {
    if (submitting) return;
    loginCancellationRequested.current = false;
    setSubmitting(true);
    setError('');
    onlineIdentity.current = '';
    knownTunnels.current = new Set();
    try {
      applyConfig(await desktop.login(address, key));
      await refreshStatus();
      configBeforeEdit.current = null;
      setEditingConfig(false);
    } catch (cause) {
      const message = errorMessage(cause, '获取连接配置失败');
      if (!loginCancellationRequested.current || message !== loginCancelledMessage) {
        setError(message);
      }
    } finally {
      setSubmitting(false);
      setCancelling(false);
    }
  }

  async function cancelLogin(): Promise<void> {
    if (!submitting || cancelling) return;
    loginCancellationRequested.current = true;
    setCancelling(true);
    setError('');
    try {
      await desktop.cancelLogin();
    } catch (cause) {
      loginCancellationRequested.current = false;
      setCancelling(false);
      setError(errorMessage(cause, '取消获取连接配置失败'));
    }
  }

  async function enable(): Promise<void> {
    if (action) return;
    setAction('enable');
    setError('');
    try {
      setStatus(await desktop.enableTunnels(tunnelSelection.selectedIdleNames));
    } catch (cause) {
      setError(errorMessage(cause, '启用隧道失败'));
      await refreshStatus();
    } finally {
      setAction(null);
    }
  }

  async function disable(): Promise<void> {
    if (action) return;
    setAction('disable');
    setError('');
    try {
      setStatus(await desktop.disableTunnels());
    } catch (cause) {
      setError(errorMessage(cause, '停用隧道失败'));
      await refreshStatus();
    } finally {
      setAction(null);
    }
  }

  const online = status.state === 'online';
  const showTunnelView = online && !editingConfig;
  const tunnelSelection = useMemo(() => {
    const idleNames: string[] = [];
    const selectedIdleNames: string[] = [];
    let enabledCount = 0;

    for (const tunnel of status.tunnels) {
      if (tunnel.state === 'idle') {
        idleNames.push(tunnel.name);
        if (selected.has(tunnel.name)) selectedIdleNames.push(tunnel.name);
      } else if (tunnel.state === 'enabled') {
        enabledCount += 1;
      }
    }

    return { enabledCount, idleNames, selectedIdleNames };
  }, [selected, status.tunnels]);

  function toggleAllIdle(): void {
    setSelected((current) => {
      // 全选仅增删当前空闲项，已启用和被占用项始终保持原状态。
      const selectAll = tunnelSelection.idleNames.some((name) => !current.has(name));
      const next = new Set(current);
      for (const name of tunnelSelection.idleNames) {
        if (selectAll) next.add(name); else next.delete(name);
      }
      return next;
    });
  }

  function editConfig(): void {
    configBeforeEdit.current = { address, key };
    setError('');
    setEditingConfig(true);
  }

  function cancelConfigEdit(): void {
    const previous = configBeforeEdit.current;
    if (previous) {
      setAddress(previous.address);
      setKey(previous.key);
    }
    configBeforeEdit.current = null;
    setError('');
    setEditingConfig(false);
  }

  return (
    <div className="app-shell">
      <main className={showTunnelView ? 'workspace' : 'workspace login-workspace'}>
        {loading ? (
          <div className="center-state"><LoaderCircle className="spin" size={22} /><span>正在启动</span></div>
        ) : showTunnelView ? (
          <section className="tunnel-view">
            <div className="view-heading">
              <div>
                <h1>隧道</h1>
                <div className="server-address">
                  <p>{status.server}</p>
                  <button
                    aria-label="修改连接配置"
                    className="edit-config-button"
                    disabled={action !== null}
                    onClick={editConfig}
                    title="修改连接配置"
                    type="button"
                  >
                    <Pencil size={13} />
                  </button>
                </div>
              </div>
            </div>

            {error && <div className="notice error" role="alert">{error}</div>}
            <TunnelList
              onToggle={(name) => setSelected((current) => {
                const next = new Set(current);
                if (next.has(name)) next.delete(name); else next.add(name);
                return next;
              })}
              selected={selected}
              tunnels={status.tunnels}
            />
            <TunnelActions
              action={action}
              enabledCount={tunnelSelection.enabledCount}
              idleCount={tunnelSelection.idleNames.length}
              onEnable={enable}
              onDisable={disable}
              onToggleAll={toggleAllIdle}
              selectedIdleCount={tunnelSelection.selectedIdleNames.length}
            />
          </section>
        ) : (
          <LoginForm
            address={address}
            error={error || (status.state === 'reconnecting' ? status.message ?? '' : '')}
            keyValue={key}
            onAddressChange={setAddress}
            onBack={editingConfig ? cancelConfigEdit : undefined}
            onCancel={cancelLogin}
            onKeyChange={setKey}
            onSubmit={login}
            state={
              cancelling
                ? 'cancelling'
                : submitting
                  ? 'fetching'
                  : status.state === 'connecting'
                    ? 'connecting'
                    : 'idle'
            }
          />
        )}
      </main>
    </div>
  );
}
