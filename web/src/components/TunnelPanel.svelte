<script lang="ts">
  import { Check, Copy, KeyRound, Pencil, Plus, RefreshCw, Save, Trash2, X } from '@lucide/svelte';
  import { generateKey, saveTunnel } from '../lib/api';
  import type { GroupConfig, ServerConfig, TunnelConfig, TunnelKind } from '../lib/types';

  export let token: string;
  export let config: ServerConfig | null | undefined;
  export let onSaved: (config: ServerConfig) => void;

  const initial: ServerConfig = config ? structuredClone(config) : {
    quic: { bind: '0.0.0.0:2333', certificate: 'certs/server.pem', private_key: 'certs/server-key.pem' },
    groups: [], tunnels: []
  };
  let draft = initial;
  let editor: 'group' | 'tunnel' | null = null;
  let editIndex = -1;
  let group: GroupConfig = { name: '', key: '' };
  let tunnel: TunnelConfig = newTunnel();
  let limit = '';
  let saving = false;
  let message = '';
  let error = '';

  function newTunnel(): TunnelConfig {
    return { name: '', group: '', kind: 'tcp', bind: '0.0.0.0:8080', limit_bps: null, max_connections: 128, max_udp_sessions: 512, udp_idle_seconds: 60 };
  }

  function openGroup(index = -1) {
    editIndex = index;
    group = index >= 0 ? { ...draft.groups[index] } : { name: '', key: '' };
    editor = 'group'; error = '';
  }

  function openTunnel(index = -1) {
    editIndex = index;
    tunnel = index >= 0 ? { ...draft.tunnels[index] } : { ...newTunnel(), group: draft.groups[0]?.name ?? '' };
    limit = tunnel.limit_bps?.toString() ?? '';
    editor = 'tunnel'; error = '';
  }

  async function refreshKey() {
    try { group.key = (await generateKey(token)).key; group = { ...group }; }
    catch (cause) { error = cause instanceof Error ? cause.message : '生成密钥失败'; }
  }

  function commitGroup() {
    if (!group.name || !group.key) { error = '名称和密钥不能为空'; return; }
    const oldName = editIndex >= 0 ? draft.groups[editIndex].name : '';
    if (editIndex >= 0) draft.groups[editIndex] = group;
    else draft.groups = [...draft.groups, group];
    if (oldName && oldName !== group.name) draft.tunnels = draft.tunnels.map((item) => item.group === oldName ? { ...item, group: group.name } : item);
    draft = { ...draft }; editor = null;
  }

  function commitTunnel() {
    tunnel.limit_bps = limit ? Number(limit) : null;
    if (!tunnel.name || !tunnel.group || !tunnel.bind) { error = '名称、分组和监听地址不能为空'; return; }
    if (editIndex >= 0) draft.tunnels[editIndex] = tunnel;
    else draft.tunnels = [...draft.tunnels, tunnel];
    draft = { ...draft }; editor = null;
  }

  function removeGroup(index: number) {
    const name = draft.groups[index].name;
    if (!confirm(`删除分组 ${name} 及其全部隧道？`)) return;
    draft.groups = draft.groups.filter((_, current) => current !== index);
    draft.tunnels = draft.tunnels.filter((item) => item.group !== name);
    draft = { ...draft };
  }

  async function persist() {
    saving = true; error = ''; message = '';
    try { draft = await saveTunnel(token, draft); onSaved(draft); message = '隧道配置已保存并触发热重载'; }
    catch (cause) { error = cause instanceof Error ? cause.message : '保存失败'; }
    finally { saving = false; }
  }

  const kindLabel: Record<TunnelKind, string> = { tcp: 'TCP', udp: 'UDP', socks5: 'SOCKS5' };
</script>

<header class="page-header"><div><p class="eyebrow">TUNNEL</p><h1>分组与隧道</h1></div><button class="primary" onclick={persist} disabled={saving}><Save size={17} />{saving ? '保存中' : '保存配置'}</button></header>
{#if message}<div class="notice success"><Check size={17} />{message}</div>{/if}
{#if error}<div class="notice error"><X size={17} />{error}</div>{/if}

<section class="section-block compact">
  <div class="section-heading"><div><h2>QUIC 监听</h2><p>服务端传输入口与 TLS 文件</p></div></div>
  <div class="form-grid three">
    <label>监听地址<input bind:value={draft.quic.bind} /></label>
    <label>证书路径<input bind:value={draft.quic.certificate} /></label>
    <label>私钥路径<input bind:value={draft.quic.private_key} /></label>
  </div>
</section>

<section class="section-block">
  <div class="section-heading"><div><h2>访问分组</h2><p>每个客户端使用独立分组密钥认证</p></div><button class="secondary" onclick={() => openGroup()}><Plus size={17} />新建分组</button></div>
  {#if draft.groups.length}
    <div class="table-wrap"><table><thead><tr><th>名称</th><th>密钥</th><th>隧道数</th><th></th></tr></thead><tbody>
      {#each draft.groups as item, index}
        <tr><td><strong>{item.name}</strong></td><td><code>{item.key.slice(0, 9)}••••••••••••</code><button class="table-icon" title="复制密钥" onclick={() => navigator.clipboard.writeText(item.key)}><Copy size={15} /></button></td><td>{draft.tunnels.filter((entry) => entry.group === item.name).length}</td><td class="actions"><button title="编辑" onclick={() => openGroup(index)}><Pencil size={16} /></button><button class="danger-icon" title="删除" onclick={() => removeGroup(index)}><Trash2 size={16} /></button></td></tr>
      {/each}
    </tbody></table></div>
  {:else}<div class="empty"><KeyRound size={24} /><strong>还没有访问分组</strong><p>创建分组后才能添加隧道。</p></div>{/if}
</section>

<section class="section-block">
  <div class="section-heading"><div><h2>隧道</h2><p>公网监听、协议类型和资源边界</p></div><button class="secondary" onclick={() => openTunnel()} disabled={!draft.groups.length}><Plus size={17} />新建隧道</button></div>
  {#if draft.tunnels.length}
    <div class="table-wrap"><table><thead><tr><th>名称</th><th>协议</th><th>分组</th><th>监听</th><th>限速</th><th></th></tr></thead><tbody>
      {#each draft.tunnels as item, index}<tr><td><strong>{item.name}</strong></td><td><span class="protocol">{kindLabel[item.kind]}</span></td><td>{item.group}</td><td><code>{item.bind}</code></td><td>{item.limit_bps ? `${item.limit_bps.toLocaleString()} B/s` : '不限'}</td><td class="actions"><button title="编辑" onclick={() => openTunnel(index)}><Pencil size={16} /></button><button class="danger-icon" title="删除" onclick={() => { draft.tunnels = draft.tunnels.filter((_, current) => current !== index); draft = { ...draft }; }}><Trash2 size={16} /></button></td></tr>{/each}
    </tbody></table></div>
  {:else}<div class="empty"><p>暂无隧道配置</p></div>{/if}
</section>

{#if editor}
  <div class="editor-overlay" role="presentation" onclick={(event) => event.currentTarget === event.target && (editor = null)}>
    <section class="editor" role="dialog" aria-modal="true">
      <header><div><p class="eyebrow">{editIndex >= 0 ? 'EDIT' : 'NEW'}</p><h2>{editor === 'group' ? '访问分组' : '隧道'}</h2></div><button class="icon-button" aria-label="关闭" onclick={() => (editor = null)}><X size={20} /></button></header>
      {#if editor === 'group'}
        <div class="editor-body"><label>分组名称<input bind:value={group.name} placeholder="office" /></label><label>256-bit 分组密钥<div class="input-action"><input bind:value={group.key} /><button title="生成新密钥" onclick={refreshKey}><RefreshCw size={17} /></button></div></label></div>
        <footer><button class="secondary" onclick={() => (editor = null)}>取消</button><button class="primary" onclick={commitGroup}>确认</button></footer>
      {:else}
        <div class="editor-body form-grid two"><label>名称<input bind:value={tunnel.name} placeholder="ssh" /></label><label>分组<select bind:value={tunnel.group}>{#each draft.groups as item}<option value={item.name}>{item.name}</option>{/each}</select></label><label>协议<select bind:value={tunnel.kind}><option value="tcp">TCP</option><option value="udp">UDP</option><option value="socks5">SOCKS5</option></select></label><label>监听地址<input bind:value={tunnel.bind} /></label><label>限速（B/s）<input type="number" min="1" bind:value={limit} placeholder="留空表示不限" /></label>{#if tunnel.kind === 'udp'}<label>最大 UDP 会话<input type="number" min="1" bind:value={tunnel.max_udp_sessions} /></label><label>UDP 空闲秒数<input type="number" min="1" bind:value={tunnel.udp_idle_seconds} /></label>{:else}<label>最大并发连接<input type="number" min="1" bind:value={tunnel.max_connections} /></label>{/if}</div>
        <footer><button class="secondary" onclick={() => (editor = null)}>取消</button><button class="primary" onclick={commitTunnel}>确认</button></footer>
      {/if}
      {#if error}<p class="form-error editor-error">{error}</p>{/if}
    </section>
  </div>
{/if}
