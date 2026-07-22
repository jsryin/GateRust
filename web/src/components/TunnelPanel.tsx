import {
  Copy,
  KeyRound,
  LogOut,
  Pencil,
  Plus,
  RefreshCw,
  Trash2
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  createGroup,
  createTunnel,
  deleteGroup,
  deleteTunnel,
  disconnectTunnelClient,
  generateKey,
  getTunnelRuntime,
  setTunnelQuic,
  updateGroup,
  updateTunnel
} from '../lib/api';
import { errorMessage } from '../lib/errors';
import type {
  GroupConfig,
  ServerConfig,
  ServerQuicConfig,
  TunnelConfig,
  TunnelKind,
  TunnelRuntimeClient,
  TunnelRuntimeState
} from '../lib/types';
import { Badge } from './ui/Badge';
import { Button } from './ui/Button';
import { ConfirmAction } from './ui/ConfirmAction';
import { Dialog, DialogBody, DialogContent, DialogFooter } from './ui/Dialog';
import { Field, Input, Select, ValueField } from './ui/Fields';
import { FormGrid, PageIntro } from './ui/Page';
import { EmptyState, Panel, PanelHeader } from './ui/Panel';
import { Notice } from './ui/Notice';
import { Table, TableCell, TableHead, TableHeader, TableRow } from './ui/Table';

interface TunnelPanelProps {
  config: ServerConfig | null | undefined;
  onSaved: (config: ServerConfig) => void;
  token: string;
}

type Editor = 'quic' | 'group' | 'tunnel' | null;

const minGroupKeyLength = 32;
const maxGroupKeyLength = 124;
const kindLabel: Record<TunnelKind, string> = { tcp: 'TCP', udp: 'UDP', socks5: 'SOCKS5' };

function defaultTunnel(): TunnelConfig {
  return {
    name: '',
    group: '',
    kind: 'tcp',
    bind: '0.0.0.0:8080',
    local_port: 8080,
    limit_bps: null,
    max_connections: 128,
    max_udp_sessions: 512,
    udp_idle_seconds: 60
  };
}

function defaultQuic(): ServerQuicConfig {
  return {
    bind: '0.0.0.0:2333',
    certificate: '/etc/gaterust/tunnel/server.pem',
    private_key: '/etc/gaterust/tunnel/server-key.pem'
  };
}

function defaultConfig(): ServerConfig {
  return {
    quic: defaultQuic(),
    groups: [],
    tunnels: []
  };
}

export function TunnelPanel({ config, onSaved, token }: TunnelPanelProps) {
  const [draft, setDraft] = useState<ServerConfig>(() => structuredClone(config ?? defaultConfig()));
  const [editor, setEditor] = useState<Editor>(null);
  const [originalName, setOriginalName] = useState<string | null>(null);
  const [quic, setQuic] = useState<ServerQuicConfig>(defaultQuic);
  const [group, setGroup] = useState<GroupConfig>({ name: '', key: '' });
  const [tunnel, setTunnel] = useState<TunnelConfig>(defaultTunnel);
  const [limit, setLimit] = useState('');
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [runtime, setRuntime] = useState<TunnelRuntimeState>({ clients: [], tunnels: [] });

  useEffect(() => {
    setDraft(structuredClone(config ?? defaultConfig()));
  }, [config]);

  const refreshRuntime = useCallback(async (signal?: AbortSignal) => {
    try {
      setRuntime(await getTunnelRuntime(token, signal));
    } catch (cause) {
      if (cause instanceof DOMException && cause.name === 'AbortError') return;
      if (!(cause instanceof Error && cause.message === '隧道模块未运行')) {
        setError(errorMessage(cause, '读取客户端状态失败'));
      }
    }
  }, [token]);

  useEffect(() => {
    const controller = new AbortController();
    let timer: number | undefined;

    async function poll() {
      await refreshRuntime(controller.signal);
      if (!controller.signal.aborted) timer = window.setTimeout(() => void poll(), 5000);
    }

    void poll();
    return () => {
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [refreshRuntime]);

  const clientsById = useMemo(
    () => new Map(runtime.clients.map((client) => [client.session_id, client])),
    [runtime.clients]
  );
  const runtimeByTunnel = useMemo(
    () => new Map(runtime.tunnels.map((item) => [item.name, item])),
    [runtime.tunnels]
  );
  const tunnelsByOwner = useMemo(() => {
    const entries = new Map<number, string[]>();
    runtime.tunnels.forEach((item) => {
      if (item.owner_session_id === null) return;
      const names = entries.get(item.owner_session_id) ?? [];
      names.push(item.name);
      entries.set(item.owner_session_id, names);
    });
    return entries;
  }, [runtime.tunnels]);
  const tunnelCountByGroup = useMemo(() => {
    const counts = new Map<string, number>();
    draft.tunnels.forEach((item) => counts.set(item.group, (counts.get(item.group) ?? 0) + 1));
    return counts;
  }, [draft.tunnels]);

  function openQuic() {
    setOriginalName(null);
    setQuic({ ...draft.quic });
    setEditor('quic');
    setError('');
  }

  function openGroup(item?: GroupConfig) {
    setOriginalName(item?.name ?? null);
    setGroup(item ? { ...item } : { name: '', key: '' });
    setEditor('group');
    setError('');
  }

  function openTunnel(item?: TunnelConfig) {
    const next = item
      ? { ...item }
      : { ...defaultTunnel(), group: draft.groups[0]?.name ?? '' };
    setOriginalName(item?.name ?? null);
    setTunnel(next);
    setLimit(next.limit_bps?.toString() ?? '');
    setEditor('tunnel');
    setError('');
  }

  async function refreshKey() {
    try {
      const result = await generateKey(token);
      setGroup((current) => ({ ...current, key: result.key }));
    } catch (cause) {
      setError(errorMessage(cause, '生成密钥失败'));
    }
  }

  async function copyGroupKey(key: string) {
    try {
      await navigator.clipboard.writeText(key);
    } catch (cause) {
      setError(errorMessage(cause, '复制密钥失败'));
    }
  }

  async function commitQuic() {
    if (!quic.bind || !quic.certificate || !quic.private_key) {
      setError('监听地址、证书路径和私钥路径不能为空');
      return;
    }
    await persistMutation(
      () => setTunnelQuic(token, quic),
      'QUIC 监听配置已保存；修改监听或 TLS 文件后请重启服务'
    );
  }

  async function commitGroup() {
    if (!group.name || !group.key) {
      setError('名称和密钥不能为空');
      return;
    }
    const keyLength = [...group.key].length;
    if (keyLength < minGroupKeyLength || keyLength > maxGroupKeyLength) {
      setError('密钥长度必须为 32 到 124 个字符');
      return;
    }

    await persistMutation(
      () => originalName ? updateGroup(token, originalName, group) : createGroup(token, group),
      originalName ? '分组已保存' : '分组已创建'
    );
  }

  async function commitTunnel() {
    const next = { ...tunnel, limit_bps: limit ? Number(limit) : null };
    if (!next.name || !next.group || !next.bind) {
      setError('名称、分组和监听地址不能为空');
      return;
    }
    if (
      next.kind !== 'socks5' &&
      next.local_port !== null &&
      (!Number.isInteger(next.local_port) || next.local_port < 1 || next.local_port > 65535)
    ) {
      setError('本地端口必须为 1 到 65535 的整数');
      return;
    }
    if (next.kind === 'socks5') next.local_port = null;

    await persistMutation(
      () => originalName ? updateTunnel(token, originalName, next) : createTunnel(token, next),
      originalName ? '隧道已保存' : '隧道已创建'
    );
  }

  async function removeGroup(name: string) {
    await persistMutation(() => deleteGroup(token, name), '分组及其隧道已删除');
  }

  async function removeTunnel(name: string) {
    await persistMutation(() => deleteTunnel(token, name), '隧道已删除');
  }

  async function disconnectClient(client: TunnelRuntimeClient) {
    try {
      await disconnectTunnelClient(token, client.session_id);
      await refreshRuntime();
    } catch (cause) {
      setError(errorMessage(cause, '下线客户端失败'));
    }
  }

  async function persistMutation(action: () => Promise<ServerConfig>, successMessage: string) {
    setSaving(true);
    setError('');
    setMessage('');
    try {
      const saved = await action();
      setDraft(saved);
      onSaved(saved);
      setMessage(successMessage);
      setEditor(null);
    } catch (cause) {
      setError(errorMessage(cause, '保存失败'));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="space-y-4">
      <PageIntro description="管理 QUIC 入口、访问分组、隧道和在线客户端" title="隧道配置" />
      {message && <Notice tone="success">{message}</Notice>}
      {error && !editor && <Notice tone="error">{error}</Notice>}

      <Panel>
        <PanelHeader
          action={(
            <Button aria-label="修改 QUIC 监听" onClick={openQuic} size="icon" title="修改" variant="ghost">
              <Pencil className="h-4 w-4" />
            </Button>
          )}
          description="服务端传输入口与 TLS 文件"
          title="QUIC 监听"
        />
        <FormGrid columns={3}>
          <ValueField label="监听地址"><code>{draft.quic.bind}</code></ValueField>
          <ValueField label="证书路径"><code>{draft.quic.certificate}</code></ValueField>
          <ValueField label="私钥路径"><code>{draft.quic.private_key}</code></ValueField>
        </FormGrid>
      </Panel>

      <Panel>
        <PanelHeader
          action={(
            <Button onClick={() => openGroup()} variant="secondary">
              <Plus className="h-4 w-4" />
              新建分组
            </Button>
          )}
          description="同一分组共享密钥与隧道权限"
          title="访问分组"
        />
        {draft.groups.length ? (
          <Table className="min-w-[660px]">
            <TableHeader>
              <TableRow>
                <TableHead>名称</TableHead>
                <TableHead>密钥</TableHead>
                <TableHead>隧道数</TableHead>
                <TableHead className="text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <tbody>
              {draft.groups.map((item) => (
                <TableRow key={item.name}>
                  <TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell>
                  <TableCell>
                    <div className="flex items-center gap-1">
                      <code className="rounded bg-[var(--bg-component)] px-1.5 py-0.5 text-xs">{item.key.slice(0, 9)}••••••••••••</code>
                      <Button
                        aria-label={`复制 ${item.name} 的密钥`}
                        onClick={() => void copyGroupKey(item.key)}
                        size="icon"
                        title="复制密钥"
                        variant="ghost"
                      >
                        <Copy className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  </TableCell>
                  <TableCell>{tunnelCountByGroup.get(item.name) ?? 0}</TableCell>
                  <TableCell>
                    <div className="flex justify-end gap-1">
                      <Button aria-label={`编辑 ${item.name}`} onClick={() => openGroup(item)} size="icon" variant="ghost">
                        <Pencil className="h-4 w-4" />
                      </Button>
                      <ConfirmAction
                        confirmLabel="删除"
                        description={`将同时删除分组 ${item.name} 下的全部隧道。`}
                        onConfirm={() => removeGroup(item.name)}
                        title={`删除分组 ${item.name}？`}
                      >
                        <Button aria-label={`删除 ${item.name}`} size="icon" variant="ghost">
                          <Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" />
                        </Button>
                      </ConfirmAction>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </tbody>
          </Table>
        ) : (
          <EmptyState description="创建分组后才能添加隧道。" icon={KeyRound} title="还没有访问分组" />
        )}
      </Panel>

      <Panel>
        <PanelHeader
          action={(
            <Button disabled={!draft.groups.length} onClick={() => openTunnel()} variant="secondary">
              <Plus className="h-4 w-4" />
              新建隧道
            </Button>
          )}
          description="公网监听、本地端口、协议类型和资源边界"
          title="隧道"
        />
        {draft.tunnels.length ? (
          <Table className="min-w-[1120px]">
            <TableHeader>
              <TableRow>
                <TableHead>名称</TableHead>
                <TableHead>协议</TableHead>
                <TableHead>分组</TableHead>
                <TableHead>监听</TableHead>
                <TableHead>本地端口</TableHead>
                <TableHead>客户端</TableHead>
                <TableHead>限速</TableHead>
                <TableHead className="text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <tbody>
              {draft.tunnels.map((item) => {
                const state = runtimeByTunnel.get(item.name);
                const owner = state?.owner_session_id == null ? undefined : clientsById.get(state.owner_session_id);
                const releasedTunnels = owner ? tunnelsByOwner.get(owner.session_id) ?? [] : [];

                return (
                  <TableRow key={item.name}>
                    <TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell>
                    <TableCell><Badge>{kindLabel[item.kind]}</Badge></TableCell>
                    <TableCell>{item.group}</TableCell>
                    <TableCell><code className="text-xs">{item.bind}</code></TableCell>
                    <TableCell>
                      {item.kind === 'socks5' ? '—' : item.local_port ?? '同监听端口'}
                    </TableCell>
                    <TableCell>
                      <div>
                        {owner ? (
                          <>
                            <div className="font-medium text-[color:var(--fg-base)]">{owner.device_id}</div>
                            <div className="txt-compact-xsmall text-[color:var(--fg-muted)]">
                              {owner.remote_address} · {new Date(owner.connected_at * 1000).toLocaleString()}
                            </div>
                          </>
                        ) : (
                          <span className="text-[color:var(--fg-muted)]">未连接</span>
                        )}
                      </div>
                    </TableCell>
                    <TableCell>{item.limit_bps ? `${item.limit_bps.toLocaleString()} B/s` : '不限'}</TableCell>
                    <TableCell>
                      <div className="flex justify-end gap-1">
                        {owner && (
                          <ConfirmAction
                            confirmLabel="下线"
                            description={`将释放隧道：${releasedTunnels.join(', ') || '无'}`}
                            onConfirm={() => disconnectClient(owner)}
                            title={`下线 ${owner.device_id}？`}
                          >
                            <Button aria-label={`下线 ${owner.device_id}`} size="icon" variant="ghost">
                              <LogOut className="h-4 w-4 text-[color:var(--tag-red-text)]" />
                            </Button>
                          </ConfirmAction>
                        )}
                        <Button aria-label={`编辑 ${item.name}`} onClick={() => openTunnel(item)} size="icon" variant="ghost">
                          <Pencil className="h-4 w-4" />
                        </Button>
                        <ConfirmAction
                          confirmLabel="删除"
                          description={`删除后，隧道 ${item.name} 将立即停止提供服务。`}
                          onConfirm={() => removeTunnel(item.name)}
                          title={`删除隧道 ${item.name}？`}
                        >
                          <Button aria-label={`删除 ${item.name}`} size="icon" variant="ghost">
                            <Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" />
                          </Button>
                        </ConfirmAction>
                      </div>
                    </TableCell>
                  </TableRow>
                );
              })}
            </tbody>
          </Table>
        ) : (
          <EmptyState title="暂无隧道配置" />
        )}
      </Panel>

      <Dialog open={editor !== null} onOpenChange={(open) => !open && !saving && setEditor(null)}>
        {editor && (
          <DialogContent
            description={editor === 'quic' || originalName ? '修改现有配置项' : '创建新的配置项'}
            title={editor === 'quic' ? 'QUIC 监听' : editor === 'group' ? '访问分组' : '隧道'}
          >
            <DialogBody>
              <div className="grid gap-4 sm:grid-cols-2">
                {editor === 'quic' ? (
                  <>
                    <Field className="sm:col-span-2" label="监听地址">
                      <Input onChange={(event) => setQuic((current) => ({ ...current, bind: event.target.value }))} value={quic.bind} />
                    </Field>
                    <Field className="sm:col-span-2" label="证书路径">
                      <Input onChange={(event) => setQuic((current) => ({ ...current, certificate: event.target.value }))} value={quic.certificate} />
                    </Field>
                    <Field className="sm:col-span-2" label="私钥路径">
                      <Input onChange={(event) => setQuic((current) => ({ ...current, private_key: event.target.value }))} value={quic.private_key} />
                    </Field>
                  </>
                ) : editor === 'group' ? (
                  <>
                    <Field className="sm:col-span-2" label="分组名称">
                      <Input onChange={(event) => setGroup((current) => ({ ...current, name: event.target.value }))} placeholder="office" value={group.name} />
                    </Field>
                    <Field className="sm:col-span-2" label="分组密钥（32-124 个字符）">
                      <div className="grid grid-cols-[minmax(0,1fr)_32px]">
                        <Input className="rounded-r-none" onChange={(event) => setGroup((current) => ({ ...current, key: event.target.value }))} value={group.key} />
                        <Button aria-label="生成新密钥" className="rounded-l-none" onClick={() => void refreshKey()} size="icon" variant="secondary">
                          <RefreshCw className="h-4 w-4" />
                        </Button>
                      </div>
                    </Field>
                  </>
                ) : (
                  <>
                    <Field label="名称">
                      <Input onChange={(event) => setTunnel((current) => ({ ...current, name: event.target.value }))} placeholder="ssh" value={tunnel.name} />
                    </Field>
                    <Field label="分组">
                      <Select onChange={(event) => setTunnel((current) => ({ ...current, group: event.target.value }))} value={tunnel.group}>
                        {draft.groups.map((item) => <option key={item.name} value={item.name}>{item.name}</option>)}
                      </Select>
                    </Field>
                    <Field label="协议">
                      <Select
                        onChange={(event) => {
                          const kind = event.target.value as TunnelKind;
                          setTunnel((current) => ({
                            ...current,
                            kind,
                            local_port: kind === 'socks5' ? null : current.local_port ?? 8080
                          }));
                        }}
                        value={tunnel.kind}
                      >
                        <option value="tcp">TCP</option>
                        <option value="udp">UDP</option>
                        <option value="socks5">SOCKS5</option>
                      </Select>
                    </Field>
                    <Field label="监听地址">
                      <Input onChange={(event) => setTunnel((current) => ({ ...current, bind: event.target.value }))} value={tunnel.bind} />
                    </Field>
                    {tunnel.kind !== 'socks5' && (
                      <Field label="本地端口">
                        <Input
                          max="65535"
                          min="1"
                          onChange={(event) => setTunnel((current) => ({
                            ...current,
                            local_port: event.target.value ? Number(event.target.value) : null
                          }))}
                          placeholder="留空则与监听端口相同"
                          type="number"
                          value={tunnel.local_port ?? ''}
                        />
                      </Field>
                    )}
                    <Field label="限速（B/s）">
                      <Input min="1" onChange={(event) => setLimit(event.target.value)} placeholder="留空表示不限" type="number" value={limit} />
                    </Field>
                    {tunnel.kind === 'udp' ? (
                      <>
                        <Field label="最大 UDP 会话">
                          <Input min="1" onChange={(event) => setTunnel((current) => ({ ...current, max_udp_sessions: Number(event.target.value) }))} type="number" value={tunnel.max_udp_sessions} />
                        </Field>
                        <Field label="UDP 空闲秒数">
                          <Input min="1" onChange={(event) => setTunnel((current) => ({ ...current, udp_idle_seconds: Number(event.target.value) }))} type="number" value={tunnel.udp_idle_seconds} />
                        </Field>
                      </>
                    ) : (
                      <Field label="最大并发连接">
                        <Input min="1" onChange={(event) => setTunnel((current) => ({ ...current, max_connections: Number(event.target.value) }))} type="number" value={tunnel.max_connections} />
                      </Field>
                    )}
                  </>
                )}
              </div>
              {error && <p className="txt-compact-small mt-4 text-[color:var(--tag-red-text)]" role="alert">{error}</p>}
            </DialogBody>
            <DialogFooter>
              <Button disabled={saving} onClick={() => setEditor(null)} variant="secondary">取消</Button>
              <Button
                disabled={saving}
                onClick={() => void (editor === 'quic' ? commitQuic() : editor === 'group' ? commitGroup() : commitTunnel())}
              >
                {saving ? '保存中' : '保存'}
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </div>
  );
}
