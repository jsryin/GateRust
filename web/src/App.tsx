import { LoaderCircle } from 'lucide-react';
import { lazy, Suspense, useCallback, useEffect, useState } from 'react';
import { ApiError, checkSession, getConfig, streamDashboard } from './lib/api';
import type { ConfigSnapshot, Dashboard as DashboardState } from './lib/types';
import { useTheme } from './hooks/useTheme';
import { AppShell, type PageId } from './components/AppShell';
import { Dashboard } from './components/Dashboard';
import { Login } from './components/Login';
import { Button } from './components/ui/Button';
import { Notice } from './components/ui/Notice';

const TunnelPanel = lazy(() => import('./components/TunnelPanel').then((module) => ({ default: module.TunnelPanel })));
const ProxyPanel = lazy(() => import('./components/ProxyPanel').then((module) => ({ default: module.ProxyPanel })));
const ClientGenerator = lazy(() => import('./components/ClientGenerator').then((module) => ({ default: module.ClientGenerator })));

export function App() {
  const [token, setToken] = useState(() => sessionStorage.getItem('gaterust_token') ?? '');
  const [active, setActive] = useState<PageId>('dashboard');
  const [config, setConfig] = useState<ConfigSnapshot>({});
  const [dashboard, setDashboard] = useState<DashboardState | null>(null);
  const [loading, setLoading] = useState(Boolean(token));
  const [fatal, setFatal] = useState('');
  const [retry, setRetry] = useState(0);
  const { theme, toggleTheme } = useTheme();

  const logout = useCallback(() => {
    sessionStorage.removeItem('gaterust_token');
    setToken('');
    setConfig({});
    setDashboard(null);
    setFatal('');
  }, []);

  useEffect(() => {
    if (!token) return;

    const controller = new AbortController();
    let disposed = false;
    let lastRevision: number | null = null;
    let refreshingConfig = false;

    async function bootstrap() {
      setLoading(true);
      setFatal('');
      try {
        await checkSession(token);
        const snapshot = await getConfig(token);
        if (disposed) return;
        setConfig(snapshot);

        void streamDashboard(token, controller.signal, (next) => {
          if (disposed) return;
          const revisionChanged = lastRevision !== null && lastRevision !== next.revision;
          lastRevision = next.revision;
          setDashboard(next);

          // 配置修订变化时只允许一个刷新请求在途，避免事件密集时重复读取。
          if (revisionChanged && !refreshingConfig) {
            refreshingConfig = true;
            void getConfig(token)
              .then((nextConfig) => {
                if (!disposed) setConfig(nextConfig);
              })
              .catch(() => undefined)
              .finally(() => {
                refreshingConfig = false;
              });
          }
        }).catch((cause: unknown) => {
          if (disposed || (cause instanceof DOMException && cause.name === 'AbortError')) return;
          if (cause instanceof ApiError && cause.status === 401) {
            logout();
            return;
          }
          setFatal(cause instanceof Error ? cause.message : '实时状态连接失败');
        });
      } catch (cause) {
        if (disposed) return;
        if (cause instanceof ApiError && cause.status === 401) logout();
        else setFatal(cause instanceof Error ? cause.message : '加载控制台失败');
      } finally {
        if (!disposed) setLoading(false);
      }
    }

    void bootstrap();
    return () => {
      disposed = true;
      controller.abort();
    };
  }, [logout, retry, token]);

  function authenticated(nextToken: string) {
    sessionStorage.setItem('gaterust_token', nextToken);
    setToken(nextToken);
  }

  if (!token) {
    return <Login onAuthenticated={authenticated} onToggleTheme={toggleTheme} theme={theme} />;
  }

  return (
    <AppShell
      active={active}
      onLogout={logout}
      onNavigate={setActive}
      onToggleTheme={toggleTheme}
      theme={theme}
    >
      {loading ? (
        <div className="flex min-h-[60vh] flex-col items-center justify-center gap-3 text-[color:var(--fg-muted)]">
          <LoaderCircle className="h-6 w-6 animate-spin" />
          <p className="txt-compact-small">载入控制台</p>
        </div>
      ) : (
        <>
          {fatal && (
            <Notice tone="error">
              <div className="flex items-center justify-between gap-3">
                <span>{fatal}</span>
                <Button className="shrink-0" onClick={() => setRetry((current) => current + 1)} size="small" variant="ghost">
                  重试
                </Button>
              </div>
            </Notice>
          )}
          <Suspense fallback={<PageLoading />}>
            {active === 'dashboard' && <Dashboard config={config} dashboard={dashboard} />}
            {active === 'tunnel' && (
              <TunnelPanel
                config={config.tunnel}
                onSaved={(tunnel) => setConfig((current) => ({ ...current, tunnel }))}
                token={token}
              />
            )}
            {active === 'proxy' && (
              <ProxyPanel
                config={config.proxy}
                onSaved={(proxy) => setConfig((current) => ({ ...current, proxy }))}
                token={token}
              />
            )}
            {active === 'client' && <ClientGenerator config={config.tunnel} token={token} />}
          </Suspense>
        </>
      )}
    </AppShell>
  );
}

function PageLoading() {
  return (
    <div className="flex min-h-48 items-center justify-center text-[color:var(--fg-muted)]">
      <LoaderCircle className="h-5 w-5 animate-spin" />
      <span className="txt-compact-small ml-2">载入页面</span>
    </div>
  );
}
