import { CirclePower, LoaderCircle } from 'lucide-react';
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
  const [action, setAction] = useState<'connect' | 'disconnect' | null>(null);
  const [error, setError] = useState('');
  const connectedIdentity = useRef('');
  const knownTunnels = useRef<Set<string>>(new Set());
  const loginCancellationRequested = useRef(false);

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
    if (status.state !== 'connected' || !status.device_id) return;
    const identity = `${status.server ?? ''}/${status.device_id}`;
    const currentNames = new Set(status.tunnels.map((tunnel) => tunnel.name));
    setSelected((current) => {
      const sameIdentity = connectedIdentity.current === identity;
      const next = sameIdentity ? new Set(current) : new Set<string>();
      let changed = !sameIdentity && current.size > 0;
      for (const tunnel of status.tunnels) {
        if (
          tunnel.state === 'connected' ||
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
    connectedIdentity.current = identity;
    knownTunnels.current = currentNames;
  }, [status]);

  async function login(): Promise<void> {
    if (submitting) return;
    loginCancellationRequested.current = false;
    setSubmitting(true);
    setError('');
    connectedIdentity.current = '';
    knownTunnels.current = new Set();
    try {
      applyConfig(await desktop.login(address, key));
      await refreshStatus();
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

  async function connect(): Promise<void> {
    if (action) return;
    setAction('connect');
    setError('');
    try {
      await desktop.connectTunnels(tunnelSelection.selectedIdleNames);
      await refreshStatus();
    } catch (cause) {
      setError(errorMessage(cause, '连接隧道失败'));
    } finally {
      setAction(null);
    }
  }

  async function disconnect(): Promise<void> {
    if (action) return;
    setAction('disconnect');
    setError('');
    try {
      await desktop.disconnectTunnels();
      await refreshStatus();
    } catch (cause) {
      setError(errorMessage(cause, '断开隧道失败'));
    } finally {
      setAction(null);
    }
  }

  const connected = status.state === 'connected';
  const tunnelSelection = useMemo(() => {
    const idleNames: string[] = [];
    const selectedIdleNames: string[] = [];
    let connectedCount = 0;

    for (const tunnel of status.tunnels) {
      if (tunnel.state === 'idle') {
        idleNames.push(tunnel.name);
        if (selected.has(tunnel.name)) selectedIdleNames.push(tunnel.name);
      } else if (tunnel.state === 'connected') {
        connectedCount += 1;
      }
    }

    return { connectedCount, idleNames, selectedIdleNames };
  }, [selected, status.tunnels]);

  function toggleAllIdle(): void {
    setSelected((current) => {
      // 全选仅增删当前空闲项，已连接和被占用项始终保持原状态。
      const selectAll = tunnelSelection.idleNames.some((name) => !current.has(name));
      const next = new Set(current);
      for (const name of tunnelSelection.idleNames) {
        if (selectAll) next.add(name); else next.delete(name);
      }
      return next;
    });
  }

  return (
    <div className="app-shell">
      <main className={connected ? 'workspace' : 'workspace login-workspace'}>
        {loading ? (
          <div className="center-state"><LoaderCircle className="spin" size={22} /><span>正在启动</span></div>
        ) : connected ? (
          <section className="tunnel-view">
            <div className="view-heading">
              <div>
                <h1>隧道</h1>
                <p>{status.server}</p>
              </div>
              <button
                className="secondary-button"
                disabled={!tunnelSelection.connectedCount || action !== null}
                onClick={() => void disconnect()}
                type="button"
              >
                {action === 'disconnect' ? <LoaderCircle className="spin" size={15} /> : <CirclePower size={15} />}
                断开
              </button>
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
              connectedCount={tunnelSelection.connectedCount}
              idleCount={tunnelSelection.idleNames.length}
              onConnect={connect}
              onDisconnect={disconnect}
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
