import {
  Copy,
  KeyRound,
  LogOut,
  Pencil,
  Plus,
  RefreshCw,
  Save,
  Trash2
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { disconnectTunnelClient, generateKey, getTunnelRuntime, saveTunnel } from '../lib/api';
import { errorMessage } from '../lib/errors';
import type {
  GroupConfig,
  ServerConfig,
  TunnelConfig,
  TunnelKind,
  TunnelRuntimeClient,
  TunnelRuntimeState
} from '../lib/types';
import { Badge } from './ui/Badge';
import { Button } from './ui/Button';
import { ConfirmAction } from './ui/ConfirmAction';
import { Dialog, DialogBody, DialogContent, DialogFooter } from './ui/Dialog';
import { Field, Input, Select } from './ui/Fields';
import { FormGrid, PageIntro } from './ui/Page';
import { EmptyState, Panel, PanelHeader } from './ui/Panel';
import { Notice } from './ui/Notice';
import { Table, TableCell, TableHead, TableHeader, TableRow } from './ui/Table';

interface TunnelPanelProps {
  config: ServerConfig | null | undefined;
  onSaved: (config: ServerConfig) => void;
  token: string;
}

type Editor = 'group' | 'tunnel' | null;

const minGroupKeyLength = 32;
const maxGroupKeyLength = 124;
const kindLabel: Record<TunnelKind, string> = { tcp: 'TCP', udp: 'UDP', socks5: 'SOCKS5' };

function defaultTunnel(): TunnelConfig {
  return {
    name: '',
    group: '',
    kind: 'tcp',
    bind: '0.0.0.0:8080',
    limit_bps: null,
    max_connections: 128,
    max_udp_sessions: 512,
    udp_idle_seconds: 60
  };
}

function defaultConfig(): ServerConfig {
  return {
    quic: {
      bind: '0.0.0.0:2333',
      certificate: '/etc/gaterust/tunnel/server.pem',
      private_key: '/etc/gaterust/tunnel/server-key.pem'
    },
    groups: [],
    tunnels: []
  };
}

export function TunnelPanel({ config, onSaved, token }: TunnelPanelProps) {
  const [draft, setDraft] = useState<ServerConfig>(() => structuredClone(config ?? defaultConfig()));
  const [editor, setEditor] = useState<Editor>(null);
  const [editIndex, setEditIndex] = useState(-1);
  const [group, setGroup] = useState<GroupConfig>({ name: '', key: '' });
  const [tunnel, setTunnel] = useState<TunnelConfig>(defaultTunnel);
  const [limit, setLimit] = useState('');
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [runtime, setRuntime] = useState<TunnelRuntimeState>({ clients: [], tunnels: [] });

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

  function openGroup(index = -1) {
    setEditIndex(index);
    setGroup(index >= 0 ? { ...draft.groups[index] } : { name: '', key: '' });
    setEditor('group');
    setError('');
  }

  function openTunnel(index = -1) {
    const next = index >= 0
      ? { ...draft.tunnels[index] }
      : { ...defaultTunnel(), group: draft.groups[0]?.name ?? '' };
    setEditIndex(index);
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

  function commitGroup() {
    if (!group.name || !group.key) {
      setError('名称和密钥不能为空');
      return;
    }
    const keyLength = [...group.key].length;
    if (keyLength < minGroupKeyLength || keyLength > maxGroupKeyLength) {
      setError('密钥长度必须为 32 到 124 个字符');
      return;
    }

    setDraft((current) => {
      const groups = [...current.groups];
      const oldName = editIndex >= 0 ? groups[editIndex].name : '';
      if (editIndex >= 0) groups[editIndex] = group;
      else groups.push(group);
      const tunnels = oldName && oldName !== group.name
        ? current.tunnels.map((item) => item.group === oldName ? { ...item, group: group.name } : item)
        : current.tunnels;
      return { ...current, groups, tunnels };
    });
    setEditor(null);
  }

  function commitTunnel() {
    const next = { ...tunnel, limit_bps: limit ? Number(limit) : null };
    if (!next.name || !next.group || !next.bind) {
      setError('名称、分组和监听地址不能为空');
      return;
    }

    setDraft((current) => {
      const tunnels = [...current.tunnels];
      if (editIndex >= 0) tunnels[editIndex] = next;
      else tunnels.push(next);
      return { ...current, tunnels };
    });
    setEditor(null);
  }

  function removeGroup(index: number) {
    const name = draft.groups[index].name;
    setDraft((current) => ({
      ...current,
      groups: current.groups.filter((_, currentIndex) => currentIndex !== index),
      tunnels: current.tunnels.filter((item) => item.group !== name)
    }));
  }

  function removeTunnel(index: number) {
    setDraft((current) => ({
      ...current,
      tunnels: current.tunnels.filter((_, currentIndex) => currentIndex !== index)
    }));
  }

  async function disconnectClient(client: TunnelRuntimeClient) {
    try {
      await disconnectTunnelClient(token, client.session_id);
      await refreshRuntime();
    } catch (cause) {
      setError(errorMessage(cause, '下线客户端失败'));
    }
  }

  async function persist() {
    setSaving(true);
    setError('');
    setMessage('');
    try {
      const saved = await saveTunnel(token, draft);
      setDraft(saved);
      onSaved(saved);
      setMessage('配置已保存；首次启用或修改 QUIC/TLS 时请重启服务');
    } catch (cause) {
      setError(errorMessage(cause, '保存失败'));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="space-y-4">
      <PageIntro
        action={(
          <Button disabled={saving} onClick={() => void persist()}>
            <Save className="h-4 w-4" />
            {saving ? '保存中' : '保存配置'}
          </Button>
        )}
        description="管理 QUIC 入口、访问分组、隧道和在线客户端"
        title="隧道配置"
      />
      {message && <Notice tone="success">{message}</Notice>}
      {error && !editor && <Notice tone="error">{error}</Notice>}

      <Panel>
        <PanelHeader description="服务端传输入口与 TLS 文件" title="QUIC 监听" />
        <FormGrid columns={3}>
          <Field label="监听地址">
            <Input
              onChange={(event) => setDraft((current) => ({ ...current, quic: { ...current.quic, bind: event.target.value } }))}
              value={draft.quic.bind}
            />
          </Field>
          <Field label="证书路径">
            <Input
              onChange={(event) => setDraft((current) => ({ ...current, quic: { ...current.quic, certificate: event.target.value } }))}
              value={draft.quic.certificate}
            />
          </Field>
          <Field label="私钥路径">
            <Input
              onChange={(event) => setDraft((current) => ({ ...current, quic: { ...current.quic, private_key: event.target.value } }))}
              value={draft.quic.private_key}
            />
          </Field>
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
              {draft.groups.map((item, index) => (
                <TableRow key={`${item.name}-${index}`}>
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
                      <Button aria-label={`编辑 ${item.name}`} onClick={() => openGroup(index)} size="icon" variant="ghost">
                        <Pencil className="h-4 w-4" />
                      </Button>
                      <ConfirmAction
                        confirmLabel="删除"
                        description={`将同时删除分组 ${item.name} 下的全部隧道。`}
                        onConfirm={() => removeGroup(index)}
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
          description="公网监听、协议类型和资源边界"
          title="隧道"
        />
        {draft.tunnels.length ? (
          <Table className="min-w-[1040px]">
            <TableHeader>
              <TableRow>
                <TableHead>名称</TableHead>
                <TableHead>协议</TableHead>
                <TableHead>分组</TableHead>
                <TableHead>监听</TableHead>
                <TableHead>客户端</TableHead>
                <TableHead>限速</TableHead>
                <TableHead className="text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <tbody>
              {draft.tunnels.map((item, index) => {
                const state = runtimeByTunnel.get(item.name);
                const owner = state?.owner_session_id == null ? undefined : clientsById.get(state.owner_session_id);
                const waiting = state?.waiting_session_ids.map((id) => clientsById.get(id)?.device_id ?? `#${id}`) ?? [];
                const releasedTunnels = owner ? tunnelsByOwner.get(owner.session_id) ?? [] : [];

                return (
                  <TableRow key={`${item.name}-${index}`}>
                    <TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell>
                    <TableCell><Badge>{kindLabel[item.kind]}</Badge></TableCell>
                    <TableCell>{item.group}</TableCell>
                    <TableCell><code className="text-xs">{item.bind}</code></TableCell>
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
                        {waiting.length > 0 && (
                          <div className="txt-compact-xsmall text-[color:var(--fg-muted)]">等待：{waiting.join(', ')}</div>
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
                        <Button aria-label={`编辑 ${item.name}`} onClick={() => openTunnel(index)} size="icon" variant="ghost">
                          <Pencil className="h-4 w-4" />
                        </Button>
                        <Button aria-label={`删除 ${item.name}`} onClick={() => removeTunnel(index)} size="icon" variant="ghost">
                          <Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" />
                        </Button>
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

      <Dialog open={editor !== null} onOpenChange={(open) => !open && setEditor(null)}>
        {editor && (
          <DialogContent
            description={editIndex >= 0 ? '修改现有配置项' : '创建新的配置项'}
            title={editor === 'group' ? '访问分组' : '隧道'}
          >
            <DialogBody>
              <div className="grid gap-4 sm:grid-cols-2">
                {editor === 'group' ? (
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
                      <Select onChange={(event) => setTunnel((current) => ({ ...current, kind: event.target.value as TunnelKind }))} value={tunnel.kind}>
                        <option value="tcp">TCP</option>
                        <option value="udp">UDP</option>
                        <option value="socks5">SOCKS5</option>
                      </Select>
                    </Field>
                    <Field label="监听地址">
                      <Input onChange={(event) => setTunnel((current) => ({ ...current, bind: event.target.value }))} value={tunnel.bind} />
                    </Field>
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
              <Button onClick={() => setEditor(null)} variant="secondary">取消</Button>
              <Button onClick={editor === 'group' ? commitGroup : commitTunnel}>确认</Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </div>
  );
}
