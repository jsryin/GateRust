<script lang="ts">
  import { Check, Globe2, Pencil, Plus, Save, ShieldCheck, Trash2, X } from '@lucide/svelte';
  import { saveProxy } from '../lib/api';
  import type { CertificateConfig, ProxyConfig, RouteConfig } from '../lib/types';

  export let token: string;
  export let config: ProxyConfig | null | undefined;
  export let onSaved: (config: ProxyConfig) => void;

  let draft: ProxyConfig = config ? structuredClone(config) : { proxy: { http_bind: '0.0.0.0:80', https_bind: '0.0.0.0:443', cache_dir: '/var/lib/gaterust/proxy/acme', max_connections: 2048 }, certificates: [], routes: [] };
  let tab: 'certificates' | 'routes' = 'certificates';
  let editor: 'certificate' | 'route' | null = null;
  let editIndex = -1;
  let certificate = newCertificate();
  let route = newRoute();
  let domains = '';
  let saving = false;
  let message = '';
  let error = '';

  function newCertificate(): CertificateConfig { return { name: '', domains: [], email: '', issuer: 'lets_encrypt', challenge: 'tls-alpn-01', production: false, cloudflare_api_token: null, cloudflare_zone_id: null, google_eab_key_id: null, google_eab_hmac_key: null, dns_propagation_seconds: 30 }; }
  function newRoute(): RouteConfig { return { name: '', host: '', path_prefix: '/', upstream: 'http://127.0.0.1:3000', certificate: null }; }
  function openCertificate(index = -1) { editIndex = index; certificate = index >= 0 ? structuredClone(draft.certificates[index]) : newCertificate(); domains = certificate.domains.join('\n'); editor = 'certificate'; error = ''; }
  function openRoute(index = -1) { editIndex = index; route = index >= 0 ? { ...draft.routes[index] } : newRoute(); editor = 'route'; error = ''; }

  function commitCertificate() {
    certificate.domains = domains.split(/[\s,]+/).filter(Boolean);
    if (certificate.challenge !== 'cloudflare-dns-01') { certificate.cloudflare_api_token = null; certificate.cloudflare_zone_id = null; }
    if (certificate.issuer !== 'google_trust_services') { certificate.google_eab_key_id = null; certificate.google_eab_hmac_key = null; }
    if (!certificate.name || !certificate.email || !certificate.domains.length) { error = '名称、域名和联系邮箱不能为空'; return; }
    if (editIndex >= 0) draft.certificates[editIndex] = certificate; else draft.certificates = [...draft.certificates, certificate];
    draft = { ...draft }; editor = null;
  }
  function commitRoute() {
    if (!route.name || !route.host || !route.upstream) { error = '名称、域名和上游地址不能为空'; return; }
    if (editIndex >= 0) draft.routes[editIndex] = route; else draft.routes = [...draft.routes, route];
    draft = { ...draft }; editor = null;
  }
  async function persist() { saving = true; error = ''; message = ''; try { draft = await saveProxy(token, draft); onSaved(draft); message = '配置已保存；首次启用或修改监听参数时请重启服务'; } catch (cause) { error = cause instanceof Error ? cause.message : '保存失败'; } finally { saving = false; } }
</script>

<header class="page-header"><div><p class="eyebrow">PROXY & SSL</p><h1>域名与 SSL</h1></div><button class="primary" onclick={persist} disabled={saving}><Save size={17} />{saving ? '保存中' : '保存配置'}</button></header>
{#if message}<div class="notice success"><Check size={17} />{message}</div>{/if}{#if error}<div class="notice error"><X size={17} />{error}</div>{/if}
<section class="section-block compact"><div class="section-heading"><div><h2>代理监听</h2><p>HTTP、HTTPS 入口与 ACME 缓存</p></div></div><div class="form-grid four"><label>HTTP 地址<input bind:value={draft.proxy.http_bind} /></label><label>HTTPS 地址<input bind:value={draft.proxy.https_bind} /></label><label>缓存目录<input bind:value={draft.proxy.cache_dir} /></label><label>最大连接数<input type="number" min="1" bind:value={draft.proxy.max_connections} /></label></div></section>

<div class="tabs"><button class:active={tab === 'certificates'} onclick={() => (tab = 'certificates')}>证书</button><button class:active={tab === 'routes'} onclick={() => (tab = 'routes')}>域名路由</button></div>
{#if tab === 'certificates'}
  <section class="section-block"><div class="section-heading"><div><h2>托管证书</h2><p>ACME 签发、DNS Provider 与续期配置</p></div><button class="secondary" onclick={() => openCertificate()}><Plus size={17} />新建证书</button></div>
  {#if draft.certificates.length}<div class="table-wrap"><table><thead><tr><th>名称</th><th>域名</th><th>签发机构</th><th>验证方式</th><th>环境</th><th></th></tr></thead><tbody>{#each draft.certificates as item, index}<tr><td><strong>{item.name}</strong></td><td>{item.domains.join(', ')}</td><td>{item.issuer === 'lets_encrypt' ? "Let's Encrypt" : 'Google Trust Services'}</td><td><code>{item.challenge}</code></td><td><span class:production={item.production} class="environment">{item.production ? '生产' : '测试'}</span></td><td class="actions"><button title="编辑" onclick={() => openCertificate(index)}><Pencil size={16} /></button><button class="danger-icon" title="删除" onclick={() => { draft.certificates = draft.certificates.filter((_, current) => current !== index); draft.routes = draft.routes.map((entry) => entry.certificate === item.name ? { ...entry, certificate: null } : entry); draft = { ...draft }; }}><Trash2 size={16} /></button></td></tr>{/each}</tbody></table></div>{:else}<div class="empty"><ShieldCheck size={24} /><strong>还没有托管证书</strong><p>添加证书后可在域名路由中直接绑定。</p></div>{/if}</section>
{:else}
  <section class="section-block"><div class="section-heading"><div><h2>域名路由</h2><p>按 Host 和路径前缀转发至上游</p></div><button class="secondary" onclick={() => openRoute()}><Plus size={17} />新建路由</button></div>
  {#if draft.routes.length}<div class="table-wrap"><table><thead><tr><th>名称</th><th>Host / Path</th><th>上游</th><th>SSL 证书</th><th></th></tr></thead><tbody>{#each draft.routes as item, index}<tr><td><strong>{item.name}</strong></td><td><code>{item.host}{item.path_prefix}</code></td><td>{item.upstream}</td><td>{item.certificate ?? '不启用'}</td><td class="actions"><button title="编辑" onclick={() => openRoute(index)}><Pencil size={16} /></button><button class="danger-icon" title="删除" onclick={() => { draft.routes = draft.routes.filter((_, current) => current !== index); draft = { ...draft }; }}><Trash2 size={16} /></button></td></tr>{/each}</tbody></table></div>{:else}<div class="empty"><Globe2 size={24} /><strong>还没有域名路由</strong><p>路由可指向本地隧道端口或公网 HTTP(S) 上游。</p></div>{/if}</section>
{/if}

{#if editor}<div class="editor-overlay" role="presentation" onclick={(event) => event.currentTarget === event.target && (editor = null)}><section class="editor wide-editor" role="dialog" aria-modal="true"><header><div><p class="eyebrow">{editIndex >= 0 ? 'EDIT' : 'NEW'}</p><h2>{editor === 'certificate' ? '托管证书' : '域名路由'}</h2></div><button class="icon-button" aria-label="关闭" onclick={() => (editor = null)}><X size={20} /></button></header>
  {#if editor === 'certificate'}<div class="editor-body form-grid two"><label>名称<input bind:value={certificate.name} /></label><label>联系邮箱<input type="email" bind:value={certificate.email} /></label><label class="span-two">域名（每行一个）<textarea rows="3" bind:value={domains}></textarea></label><label>签发机构<select bind:value={certificate.issuer}><option value="lets_encrypt">Let's Encrypt</option><option value="google_trust_services">Google Trust Services</option></select></label><label>验证方式<select bind:value={certificate.challenge}><option value="http-01">HTTP-01</option><option value="tls-alpn-01">TLS-ALPN-01</option><option value="cloudflare-dns-01">Cloudflare DNS-01</option></select></label><label class="toggle-line"><input type="checkbox" bind:checked={certificate.production} /><span>使用生产环境证书</span></label>{#if certificate.challenge === 'cloudflare-dns-01'}<label>Cloudflare API Token<input type="password" bind:value={certificate.cloudflare_api_token} /></label><label>Cloudflare Zone ID<input bind:value={certificate.cloudflare_zone_id} /></label><label>DNS 传播秒数<input type="number" min="1" max="600" bind:value={certificate.dns_propagation_seconds} /></label>{/if}{#if certificate.issuer === 'google_trust_services'}<label>Google EAB Key ID<input bind:value={certificate.google_eab_key_id} /></label><label>Google EAB HMAC Key<input type="password" bind:value={certificate.google_eab_hmac_key} /></label>{/if}</div><footer><button class="secondary" onclick={() => (editor = null)}>取消</button><button class="primary" onclick={commitCertificate}>确认</button></footer>
  {:else}<div class="editor-body form-grid two"><label>名称<input bind:value={route.name} /></label><label>域名<input bind:value={route.host} placeholder="example.com" /></label><label>路径前缀<input bind:value={route.path_prefix} /></label><label>上游地址<input bind:value={route.upstream} /></label><label>SSL 证书<select bind:value={route.certificate}><option value={null}>不启用</option>{#each draft.certificates as item}<option value={item.name}>{item.name}</option>{/each}</select></label></div><footer><button class="secondary" onclick={() => (editor = null)}>取消</button><button class="primary" onclick={commitRoute}>确认</button></footer>{/if}
  {#if error}<p class="form-error editor-error">{error}</p>{/if}</section></div>{/if}
