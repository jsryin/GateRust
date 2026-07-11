<script lang="ts">
  import { onMount } from 'svelte';
  import ClientGenerator from './components/ClientGenerator.svelte';
  import DashboardView from './components/Dashboard.svelte';
  import Login from './components/Login.svelte';
  import ProxyPanel from './components/ProxyPanel.svelte';
  import Sidebar from './components/Sidebar.svelte';
  import TunnelPanel from './components/TunnelPanel.svelte';
  import { ApiError, checkSession, getConfig, streamDashboard } from './lib/api';
  import type { ConfigSnapshot, Dashboard } from './lib/types';

  let token = sessionStorage.getItem('gaterust_token') ?? '';
  let active = 'dashboard';
  let config: ConfigSnapshot = {};
  let dashboard: Dashboard | null = null;
  let loading = Boolean(token);
  let fatal = '';
  let stream: AbortController | null = null;

  onMount(() => { if (token) bootstrap(); return () => stream?.abort(); });

  async function bootstrap() {
    loading = true; fatal = '';
    try {
      await checkSession(token);
      config = await getConfig(token);
      connectStream();
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 401) logout();
      else fatal = cause instanceof Error ? cause.message : '加载控制台失败';
    } finally { loading = false; }
  }

  function connectStream() {
    stream?.abort(); stream = new AbortController();
    streamDashboard(token, stream.signal, (next) => { const changed = dashboard && dashboard.revision !== next.revision; dashboard = next; if (changed) getConfig(token).then((nextConfig) => (config = nextConfig)).catch(() => undefined); })
      .catch((cause) => { if (!(cause instanceof DOMException && cause.name === 'AbortError')) fatal = cause instanceof Error ? cause.message : '实时状态连接失败'; });
  }

  function authenticated(nextToken: string) { token = nextToken; sessionStorage.setItem('gaterust_token', token); bootstrap(); }
  function logout() { stream?.abort(); token = ''; config = {}; dashboard = null; sessionStorage.removeItem('gaterust_token'); }
</script>

{#if !token}
  <Login onAuthenticated={authenticated} />
{:else}
  <div class="app-shell">
    <Sidebar {active} onNavigate={(page) => (active = page)} onLogout={logout} />
    <main class="content">
      {#if loading}<div class="loading"><span></span><p>载入控制台</p></div>
      {:else}
        {#if fatal}<div class="notice error top-notice">{fatal}<button onclick={() => { fatal = ''; bootstrap(); }}>重试</button></div>{/if}
        {#if active === 'dashboard'}<DashboardView {dashboard} {config} />
        {:else if active === 'tunnel'}<TunnelPanel {token} config={config.tunnel} onSaved={(tunnel) => (config = { ...config, tunnel })} />
        {:else if active === 'proxy'}<ProxyPanel {token} config={config.proxy} onSaved={(proxy) => (config = { ...config, proxy })} />
        {:else}<ClientGenerator {token} config={config.tunnel} />{/if}
      {/if}
    </main>
  </div>
{/if}
