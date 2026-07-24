import { CirclePower, LoaderCircle } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { LoginForm } from './components/LoginForm';
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
  const [action, setAction] = useState<'connect' | 'disconnect' | null>(null);
  const [error, setError] = useState('');
  const connectedIdentity = useRef('');
  const knownTunnels = useRef<Set<string>>(new Set());

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
      const next = connectedIdentity.current === identity ? new Set(current) : new Set<string>();
      for (const tunnel of status.tunnels) {
        if (
          tunnel.state === 'connected' ||
          (tunnel.state === 'idle' && !knownTunnels.current.has(tunnel.name))
        ) {
          next.add(tunnel.name);
        }
        if (tunnel.state === 'occupied') next.delete(tunnel.name);
      }
      for (const name of next) {
        if (!currentNames.has(name)) next.delete(name);
      }
      return next;
    });
    connectedIdentity.current = identity;
    knownTunnels.current = currentNames;
  }, [status]);

  async function login(): Promise<void> {
    if (submitting) return;
    setSubmitting(true);
    setError('');
    connectedIdentity.current = '';
    knownTunnels.current = new Set();
    try {
      applyConfig(await desktop.login(address, key));
      await refreshStatus();
    } catch (cause) {
      setError(errorMessage(cause, '登录失败'));
    } finally {
      setSubmitting(false);
    }
  }

  async function connect(): Promise<void> {
    if (action) return;
    setAction('connect');
    setError('');
    try {
      await desktop.connectTunnels([...selected]);
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
  const selectedIdleCount = status.tunnels.filter(
    (tunnel) => tunnel.state === 'idle' && selected.has(tunnel.name)
  ).length;
  const connectedCount = status.tunnels.filter((tunnel) => tunnel.state === 'connected').length;

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
                disabled={!connectedCount || action !== null}
                onClick={() => void disconnect()}
                type="button"
              >
                {action === 'disconnect' ? <LoaderCircle className="spin" size={15} /> : <CirclePower size={15} />}
                断开全部
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
            <div className="action-bar">
              <span>{selectedIdleCount ? `已选择 ${selectedIdleCount} 条空闲隧道` : `${connectedCount} 条已连接`}</span>
              <button
                className="primary-button"
                disabled={!selected.size || !selectedIdleCount || action !== null}
                onClick={() => void connect()}
                type="button"
              >
                {action === 'connect' && <LoaderCircle className="spin" size={16} />}
                连接
              </button>
            </div>
          </section>
        ) : (
          <LoginForm
            address={address}
            error={error || (status.state === 'reconnecting' ? status.message ?? '' : '')}
            keyValue={key}
            onAddressChange={setAddress}
            onKeyChange={setKey}
            onSubmit={login}
            pending={submitting || status.state === 'connecting'}
          />
        )}
      </main>
    </div>
  );
}
