<script lang="ts">
  import { Cable, CircleCheck, CircleOff, KeyRound, Route } from '@lucide/svelte';
  import type { ConfigSnapshot, Dashboard } from '../lib/types';

  export let dashboard: Dashboard | null;
  export let config: ConfigSnapshot;

  $: metrics = [
    { label: '分组', value: dashboard?.groups ?? config.tunnel?.groups.length ?? 0, icon: KeyRound },
    { label: '隧道', value: dashboard?.tunnels ?? config.tunnel?.tunnels.length ?? 0, icon: Cable },
    { label: '托管证书', value: dashboard?.certificates ?? config.proxy?.certificates.length ?? 0, icon: CircleCheck },
    { label: '代理路由', value: dashboard?.routes ?? config.proxy?.routes.length ?? 0, icon: Route }
  ];
</script>

<header class="page-header"><div><p class="eyebrow">OVERVIEW</p><h1>仪表盘</h1></div><span class="live"><i></i>实时状态</span></header>
<section class="metric-grid">
  {#each metrics as metric}
    <article class="metric"><div><p>{metric.label}</p><strong>{metric.value}</strong></div><metric.icon size={20} /></article>
  {/each}
</section>
<section class="section-block">
  <div class="section-heading"><div><h2>模块状态</h2><p>当前进程的启用状态与配置载入情况</p></div></div>
  <div class="status-list">
    <div class="status-row">
      <span class:enabled={dashboard?.tunnel_enabled}>{#if dashboard?.tunnel_enabled}<CircleCheck size={18} />{:else}<CircleOff size={18} />{/if}</span>
      <div><strong>QUIC 内网穿透</strong><p>{config.tunnel ? `${config.tunnel.quic.bind} · 已载入配置` : '尚未创建配置'}</p></div>
      <b class:success={dashboard?.tunnel_enabled}>{dashboard?.tunnel_enabled ? '运行中' : '未启用'}</b>
    </div>
    <div class="status-row">
      <span class:enabled={dashboard?.proxy_enabled}>{#if dashboard?.proxy_enabled}<CircleCheck size={18} />{:else}<CircleOff size={18} />{/if}</span>
      <div><strong>反向代理与自动 SSL</strong><p>{config.proxy ? `${config.proxy.proxy.http_bind} / ${config.proxy.proxy.https_bind}` : '尚未创建配置'}</p></div>
      <b class:success={dashboard?.proxy_enabled}>{dashboard?.proxy_enabled ? '运行中' : '未启用'}</b>
    </div>
  </div>
</section>
