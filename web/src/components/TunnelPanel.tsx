import { Pencil, RefreshCw } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
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
import { copyText } from '../lib/clipboard';
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
import { Button } from './ui/Button';
import { Dialog, DialogBody, DialogContent, DialogFooter } from './ui/Dialog';
import { Field, Input, Select, ValueField } from './ui/Fields';
import { FormGrid, PageIntro } from './ui/Page';
import { Panel, PanelHeader } from './ui/Panel';
import { Notice } from './ui/Notice';
import { TunnelGroupList } from './TunnelGroupList';

interface TunnelPanelProps {
  config: ServerConfig | null | undefined;
  onSaved: (config: ServerConfig) => void;
  token: string;
}

type Editor = 'quic' | 'group' | 'tunnel' | null;

const minGroupKeyLength = 32;
const maxGroupKeyLength = 124;
const maxDataStreams = 512;
const maxUdpSessions = 128;
const maxUdpIdleSeconds = 3600;
const bytesPerKilobyte = 1024;

function defaultTunnel(): TunnelConfig {
  return {
    name: '',
    group: '',
    kind: 'tcp',
    bind: '0.0.0.0:8080',
    local_ip: '127.0.0.1',
    local_port: 8080,
    limit_bps: null,
    max_connections: 128,
    max_udp_sessions: 128,
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
  const [limitKilobytes, setLimitKilobytes] = useState('');
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [runtime, setRuntime] = useState<TunnelRuntimeState>({
    clients: [],
    tunnels: [],
    config_status: { revision: 0, last_apply_error: null }
  });

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

  function openTunnel(item?: TunnelConfig, groupName?: string) {
    const next = item
      ? { ...item }
      : { ...defaultTunnel(), group: groupName ?? draft.groups[0]?.name ?? '' };
    setOriginalName(item?.name ?? null);
    setTunnel(next);
    setLimitKilobytes(next.limit_bps === null ? '' : (next.limit_bps / bytesPerKilobyte).toString());
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
      await copyText(key);
      setError('');
      setMessage('分组密钥已复制');
    } catch (cause) {
      setMessage('');
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
      'QUIC 监听配置已应用'
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
    // 配置和 API 保持字节单位，仅在界面边界换算，避免给转发热路径增加开销。
    const limitBps = limitKilobytes
      ? Math.round(Number(limitKilobytes) * bytesPerKilobyte)
      : null;
    const next = { ...tunnel, limit_bps: limitBps };
    if (!next.name || !next.group || !next.bind) {
      setError('名称、分组和监听地址不能为空');
      return;
    }
    if (next.kind !== 'socks5' && !next.local_ip) {
      setError('本地 IP 不能为空');
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
    if (next.limit_bps !== null && (!Number.isSafeInteger(next.limit_bps) || next.limit_bps < 1)) {
      setError('限速必须为有效的正数（KB/s）');
      return;
    }
    if (next.kind === 'udp') {
      if (!Number.isInteger(next.max_udp_sessions) || next.max_udp_sessions < 1 || next.max_udp_sessions > maxUdpSessions) {
        setError(`最大 UDP 会话必须为 1 到 ${maxUdpSessions} 的整数`);
        return;
      }
      if (!Number.isInteger(next.udp_idle_seconds) || next.udp_idle_seconds < 1 || next.udp_idle_seconds > maxUdpIdleSeconds) {
        setError(`UDP 空闲秒数必须为 1 到 ${maxUdpIdleSeconds} 的整数`);
        return;
      }
    } else if (!Number.isInteger(next.max_connections) || next.max_connections < 1 || next.max_connections > maxDataStreams) {
      setError(`最大并发连接必须为 1 到 ${maxDataStreams} 的整数`);
      return;
    }
    if (next.kind === 'socks5' && !isLoopbackBind(next.bind)) {
      setError('未配置认证的 SOCKS5 只能监听 127.0.0.0/8 或 ::1');
      return;
    }

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
      {runtime.config_status.last_apply_error && (
        <Notice tone="error">最近一次运行时应用失败：{runtime.config_status.last_apply_error}</Notice>
      )}
      <Panel>
        <PanelHeader
          action={(
            <Button aria-label="修改 QUIC 监听" onClick={openQuic} size="icon" title="修改" variant="ghost">
              <Pencil className="h-4 w-4" />
            </Button>
          )}
          title="QUIC 监听"
        />
        <FormGrid columns={3}>
          <ValueField label="监听地址"><code>{draft.quic.bind}</code></ValueField>
          <ValueField label="证书路径"><code>{draft.quic.certificate}</code></ValueField>
          <ValueField label="私钥路径"><code>{draft.quic.private_key}</code></ValueField>
        </FormGrid>
      </Panel>

      <TunnelGroupList
        groups={draft.groups}
        runtime={runtime}
        tunnels={draft.tunnels}
        onCopyGroupKey={copyGroupKey}
        onCreateGroup={() => openGroup()}
        onCreateTunnel={(groupName) => openTunnel(undefined, groupName)}
        onDeleteGroup={removeGroup}
        onDeleteTunnel={removeTunnel}
        onDisconnectClient={disconnectClient}
        onEditGroup={openGroup}
        onEditTunnel={openTunnel}
      />

      <Dialog open={editor !== null} onOpenChange={(open) => !open && !saving && setEditor(null)}>
        {editor && (
          <DialogContent
            description={editor === 'quic' || (editor === 'tunnel' && originalName) ? '修改现有配置项' : undefined}
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
                        <Button aria-label="生成新密钥" className="h-8 w-8 rounded-l-none" onClick={() => void refreshKey()} size="icon" variant="secondary">
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
                      <>
                        <Field label="本地 IP">
                          <Input
                            onChange={(event) => setTunnel((current) => ({ ...current, local_ip: event.target.value }))}
                            placeholder="127.0.0.1 或 localhost"
                            value={tunnel.local_ip}
                          />
                        </Field>
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
                      </>
                    )}
                    <Field label="限速（KB/s）">
                      <Input
                        min={1 / bytesPerKilobyte}
                        onChange={(event) => setLimitKilobytes(event.target.value)}
                        placeholder="留空表示不限"
                        step="any"
                        type="number"
                        value={limitKilobytes}
                      />
                    </Field>
                    {tunnel.kind === 'udp' ? (
                      <>
                        <Field label="最大 UDP 会话">
                          <Input max={maxUdpSessions} min="1" onChange={(event) => setTunnel((current) => ({ ...current, max_udp_sessions: Number(event.target.value) }))} type="number" value={tunnel.max_udp_sessions} />
                        </Field>
                        <Field label="UDP 空闲秒数">
                          <Input max={maxUdpIdleSeconds} min="1" onChange={(event) => setTunnel((current) => ({ ...current, udp_idle_seconds: Number(event.target.value) }))} type="number" value={tunnel.udp_idle_seconds} />
                        </Field>
                      </>
                    ) : (
                      <Field label="最大并发连接">
                        <Input max={maxDataStreams} min="1" onChange={(event) => setTunnel((current) => ({ ...current, max_connections: Number(event.target.value) }))} type="number" value={tunnel.max_connections} />
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

function isLoopbackBind(bind: string): boolean {
  return /^127(?:\.\d{1,3}){3}:\d+$/.test(bind.trim()) || /^\[::1\]:\d+$/.test(bind.trim());
}
