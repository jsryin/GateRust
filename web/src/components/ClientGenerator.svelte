<script lang="ts">
  import { Check, Clipboard, Download, FileCode2, WandSparkles, X } from '@lucide/svelte';
  import { generateClientConfig } from '../lib/api';
  import type { ClientService, ServerConfig } from '../lib/types';

  export let token: string;
  export let config: ServerConfig | null | undefined;

  let group = config?.groups[0]?.name ?? '';
  let serverAddress = config?.quic.bind.replace('0.0.0.0', 'server.example.com') ?? 'server.example.com:2333';
  let serverName = 'server.example.com';
  let caCertificate = 'server.pem';
  let services: ClientService[] = config?.tunnels.map((tunnel) => ({ name: tunnel.name, kind: tunnel.kind, target: tunnel.kind === 'socks5' ? null : '127.0.0.1:8080' })) ?? [];
  let result = '';
  let error = '';
  let copied = false;

  async function generate() {
    error = '';
    try { result = (await generateClientConfig(token, { group, server_address: serverAddress, server_name: serverName, ca_certificate: caCertificate, services })).toml; }
    catch (cause) { error = cause instanceof Error ? cause.message : '生成失败'; }
  }
  async function copy() { await navigator.clipboard.writeText(result); copied = true; setTimeout(() => (copied = false), 1800); }
  function download() { const url = URL.createObjectURL(new Blob([result], { type: 'text/plain' })); const anchor = document.createElement('a'); anchor.href = url; anchor.download = 'client.toml'; anchor.click(); URL.revokeObjectURL(url); }
</script>

<header class="page-header"><div><p class="eyebrow">CLIENT</p><h1>客户端配置</h1></div></header>
{#if !config || !config.groups.length}
  <section class="section-block"><div class="empty"><FileCode2 size={26} /><strong>需要先创建访问分组</strong><p>客户端配置包含所选分组的认证密钥。</p></div></section>
{:else}
  {#if error}<div class="notice error"><X size={17} />{error}</div>{/if}
  <section class="section-block compact"><div class="section-heading"><div><h2>连接信息</h2><p>用于建立 QUIC 控制连接</p></div></div><div class="form-grid four"><label>访问分组<select bind:value={group}>{#each config.groups as item}<option value={item.name}>{item.name}</option>{/each}</select></label><label>服务器地址<input bind:value={serverAddress} /></label><label>TLS 服务器名称<input bind:value={serverName} /></label><label>CA 证书路径<input bind:value={caCertificate} /></label></div></section>
  <section class="section-block"><div class="section-heading"><div><h2>本地服务</h2><p>为服务端隧道指定此客户端上的目标地址</p></div></div><div class="service-list">{#each services as service, index}<div class="service-row"><div><strong>{service.name}</strong><span class="protocol">{service.kind.toUpperCase()}</span></div>{#if service.kind !== 'socks5'}<label>目标地址<input bind:value={service.target} placeholder="127.0.0.1:8080" /></label>{:else}<p>按请求动态连接目标</p>{/if}<label class="toggle-line"><input type="checkbox" checked oninput={(event) => { if (!(event.currentTarget as HTMLInputElement).checked) services = services.filter((_, current) => current !== index); }} /><span>包含</span></label></div>{/each}</div><div class="section-actions"><button class="primary" onclick={generate}><WandSparkles size={17} />生成配置</button></div></section>
  {#if result}<section class="section-block output"><div class="section-heading"><div><h2>client.toml</h2><p>密钥仅展示在当前登录会话中</p></div><div class="button-row"><button class="secondary icon-text" onclick={copy}>{#if copied}<Check size={16} />已复制{:else}<Clipboard size={16} />复制{/if}</button><button class="secondary icon-text" onclick={download}><Download size={16} />下载</button></div></div><pre>{result}</pre></section>{/if}
{/if}
