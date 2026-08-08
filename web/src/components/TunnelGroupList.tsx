import {
  ChevronRight,
  Copy,
  KeyRound,
  LogOut,
  Pencil,
  Plus,
  Search,
  Trash2
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import type {
  GroupConfig,
  TunnelConfig,
  TunnelKind,
  TunnelRuntimeClient,
  TunnelRuntimeState
} from '../lib/types';
import { tunnelLocalTarget } from '../lib/tunnels';
import { Badge } from './ui/Badge';
import { Button } from './ui/Button';
import { ConfirmAction } from './ui/ConfirmAction';
import { Input, Select } from './ui/Fields';
import { EmptyState, Panel, PanelHeader } from './ui/Panel';

interface TunnelGroupListProps {
  groups: GroupConfig[];
  runtime: TunnelRuntimeState;
  tunnels: TunnelConfig[];
  onCopyGroupKey: (key: string) => void | Promise<void>;
  onCreateGroup: () => void;
  onCreateTunnel: (group: string) => void;
  onDeleteGroup: (name: string) => void | Promise<void>;
  onDeleteTunnel: (name: string) => void | Promise<void>;
  onDisconnectClient: (tunnel: TunnelConfig, client: TunnelRuntimeClient) => void | Promise<void>;
  onEditGroup: (group: GroupConfig) => void;
  onEditTunnel: (tunnel: TunnelConfig) => void;
}

type StatusFilter = 'all' | 'online' | 'offline';
type KindFilter = 'all' | TunnelKind;

interface TunnelEntry {
  config: TunnelConfig;
  owner?: TunnelRuntimeClient;
}

interface GroupView {
  group: GroupConfig;
  onlineClientCount: number;
  totalTunnelCount: number;
  tunnels: TunnelEntry[];
}

const expandedGroupsStorageKey = 'gaterust_tunnel_expanded_groups';
const kindLabel: Record<TunnelKind, string> = { tcp: 'TCP', udp: 'UDP', socks5: 'SOCKS5' };
const bytesPerKilobyte = 1024;

export function TunnelGroupList({
  groups,
  runtime,
  tunnels,
  onCopyGroupKey,
  onCreateGroup,
  onCreateTunnel,
  onDeleteGroup,
  onDeleteTunnel,
  onDisconnectClient,
  onEditGroup,
  onEditTunnel
}: TunnelGroupListProps) {
  const [query, setQuery] = useState('');
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all');
  const [kindFilter, setKindFilter] = useState<KindFilter>('all');
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(() => initialExpandedGroups(groups));
  const knownGroups = useRef(new Set(groups.map((group) => group.name)));
  const lastFilterKey = useRef('');

  useEffect(() => {
    const names = new Set(groups.map((group) => group.name));
    setExpandedGroups((current) => {
      const next = new Set([...current].filter((name) => names.has(name)));
      for (const name of names) {
        if (!knownGroups.current.has(name)) next.add(name);
      }
      return sameSet(current, next) ? current : next;
    });
    knownGroups.current = names;
  }, [groups]);

  useEffect(() => {
    try {
      sessionStorage.setItem(expandedGroupsStorageKey, JSON.stringify([...expandedGroups]));
    } catch {
      // 浏览器禁用存储时仅不保留展开状态，不影响配置管理。
    }
  }, [expandedGroups]);

  const runtimeIndex = useMemo(() => {
    const clientsById = new Map(runtime.clients.map((client) => [client.session_id, client]));
    const clientCountByGroup = new Map<string, number>();
    const ownerByTunnel = new Map<string, TunnelRuntimeClient>();

    runtime.clients.forEach((client) => {
      clientCountByGroup.set(client.group, (clientCountByGroup.get(client.group) ?? 0) + 1);
    });
    runtime.tunnels.forEach((tunnel) => {
      if (tunnel.owner_session_id === null) return;
      const owner = clientsById.get(tunnel.owner_session_id);
      if (owner) ownerByTunnel.set(tunnel.name, owner);
    });

    return { clientCountByGroup, ownerByTunnel };
  }, [runtime.clients, runtime.tunnels]);

  const views = useMemo(() => {
    // 先按分组建立索引，避免渲染每个分组时重复扫描全部隧道。
    const tunnelsByGroup = new Map<string, TunnelConfig[]>();
    tunnels.forEach((tunnel) => {
      const entries = tunnelsByGroup.get(tunnel.group) ?? [];
      entries.push(tunnel);
      tunnelsByGroup.set(tunnel.group, entries);
    });

    const normalizedQuery = query.trim().toLowerCase();
    const hasTunnelFilter = statusFilter !== 'all' || kindFilter !== 'all';

    return groups.flatMap<GroupView>((group) => {
      const groupTunnels = tunnelsByGroup.get(group.name) ?? [];
      const groupMatches = normalizedQuery !== '' && group.name.toLowerCase().includes(normalizedQuery);
      const matchingTunnels = groupTunnels.flatMap<TunnelEntry>((tunnel) => {
        const owner = runtimeIndex.ownerByTunnel.get(tunnel.name);
        if (statusFilter === 'online' && !owner) return [];
        if (statusFilter === 'offline' && owner) return [];
        if (kindFilter !== 'all' && tunnel.kind !== kindFilter) return [];

        const tunnelMatches = normalizedQuery === '' || groupMatches || [
          tunnel.name,
          tunnel.bind,
          tunnel.local_ip,
          kindLabel[tunnel.kind],
          tunnel.local_port?.toString() ?? '',
          owner?.device_id ?? '',
          owner?.remote_address ?? ''
        ].some((value) => value.toLowerCase().includes(normalizedQuery));

        return tunnelMatches ? [{ config: tunnel, owner }] : [];
      });

      const showEmptyGroup = !hasTunnelFilter && (
        normalizedQuery === '' || groupMatches
      );
      if (!matchingTunnels.length && !showEmptyGroup) return [];

      return [{
        group,
        onlineClientCount: runtimeIndex.clientCountByGroup.get(group.name) ?? 0,
        totalTunnelCount: groupTunnels.length,
        tunnels: matchingTunnels
      }];
    });
  }, [groups, kindFilter, query, runtimeIndex, statusFilter, tunnels]);

  const filtering = query.trim() !== '' || statusFilter !== 'all' || kindFilter !== 'all';
  const filterKey = `${query}\0${statusFilter}\0${kindFilter}`;

  useEffect(() => {
    if (!filtering) {
      lastFilterKey.current = '';
      return;
    }
    if (lastFilterKey.current === filterKey) return;
    lastFilterKey.current = filterKey;
    setExpandedGroups((current) => {
      const next = new Set(current);
      views.forEach((view) => next.add(view.group.name));
      return sameSet(current, next) ? current : next;
    });
    // 搜索或筛选变化时展开命中分组，此后仍允许用户手动折叠。
  }, [filterKey, filtering, views]);

  function toggleGroup(name: string) {
    setExpandedGroups((current) => {
      const next = new Set(current);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  }

  return (
    <Panel>
      <PanelHeader
        action={(
          <Button onClick={onCreateGroup} variant="secondary">
            <Plus className="h-4 w-4" />
            新建分组
          </Button>
        )}
        title="分组与隧道"
      />

      {groups.length ? (
        <>
          <div className="flex flex-col gap-3 border-b border-[color:var(--border-base)] px-5 py-3 sm:flex-row sm:items-center sm:px-6">
            <label className="relative min-w-0 flex-1 sm:max-w-sm">
              <span className="sr-only">搜索分组与隧道</span>
              <Search className="pointer-events-none absolute left-2.5 top-2 h-4 w-4 text-[color:var(--fg-muted)]" />
              <Input
                className="pl-8"
                onChange={(event) => setQuery(event.target.value)}
                placeholder="搜索分组、隧道、监听或客户端"
                type="search"
                value={query}
              />
            </label>
            <div className="grid grid-cols-2 gap-2 sm:flex">
              <label>
                <span className="sr-only">连接状态</span>
                <Select
                  aria-label="连接状态"
                  className="sm:w-28"
                  onChange={(event) => setStatusFilter(event.target.value as StatusFilter)}
                  value={statusFilter}
                >
                  <option value="all">全部状态</option>
                  <option value="online">在线</option>
                  <option value="offline">未连接</option>
                </Select>
              </label>
              <label>
                <span className="sr-only">隧道协议</span>
                <Select
                  aria-label="隧道协议"
                  className="sm:w-28"
                  onChange={(event) => setKindFilter(event.target.value as KindFilter)}
                  value={kindFilter}
                >
                  <option value="all">全部协议</option>
                  <option value="tcp">TCP</option>
                  <option value="udp">UDP</option>
                  <option value="socks5">SOCKS5</option>
                </Select>
              </label>
            </div>
          </div>

          {views.length ? views.map((view) => {
            const expanded = expandedGroups.has(view.group.name);
            const regionId = `tunnel-group-${encodeURIComponent(view.group.name)}`;

            return (
              <section className="border-b border-[color:var(--border-base)] last:border-b-0" key={view.group.name}>
                <div className="flex min-h-14 items-center gap-2 bg-[var(--bg-component)] px-3 py-2 sm:px-5">
                  <button
                    aria-controls={regionId}
                    aria-expanded={expanded}
                    className="group flex min-w-0 flex-1 items-center gap-2 rounded-md px-1 py-1 text-left outline-none focus-visible:shadow-[var(--borders-focus)]"
                    onClick={() => toggleGroup(view.group.name)}
                    type="button"
                  >
                    <ChevronRight className={`h-4 w-4 shrink-0 text-[color:var(--fg-muted)] transition-transform ${expanded ? 'rotate-90' : ''}`} />
                    <span className="min-w-0">
                      <span className="txt-compact-small-plus block truncate text-[color:var(--fg-base)]">{view.group.name}</span>
                      <span className="txt-compact-xsmall block truncate text-[color:var(--fg-muted)]">
                        {view.totalTunnelCount} 条隧道 · {view.onlineClientCount ? `${view.onlineClientCount} 个在线客户端` : '无在线客户端'}
                      </span>
                    </span>
                  </button>

                  <code className="txt-compact-xsmall hidden rounded bg-[var(--bg-base)] px-2 py-0.5 text-[color:var(--fg-muted)] lg:block">
                    {view.group.key.slice(0, 8)}••••••••
                  </code>
                  <div className="flex shrink-0 items-center gap-1">
                    <Button
                      aria-label={`复制 ${view.group.name} 的密钥`}
                      onClick={() => void onCopyGroupKey(view.group.key)}
                      size="icon"
                      title="复制密钥"
                      variant="ghost"
                    >
                      <Copy className="h-3.5 w-3.5" />
                    </Button>
                    <Button
                      aria-label={`在 ${view.group.name} 中新建隧道`}
                      onClick={() => onCreateTunnel(view.group.name)}
                      size="icon"
                      title="新建隧道"
                      variant="ghost"
                    >
                      <Plus className="h-4 w-4" />
                    </Button>
                    <Button
                      aria-label={`编辑 ${view.group.name}`}
                      onClick={() => onEditGroup(view.group)}
                      size="icon"
                      title="编辑分组"
                      variant="ghost"
                    >
                      <Pencil className="h-4 w-4" />
                    </Button>
                    <ConfirmAction
                      confirmLabel="删除"
                      description={`将同时删除分组 ${view.group.name} 下的 ${view.totalTunnelCount} 条隧道。`}
                      onConfirm={() => onDeleteGroup(view.group.name)}
                      title={`删除分组 ${view.group.name}？`}
                    >
                      <Button aria-label={`删除 ${view.group.name}`} size="icon" title="删除分组" variant="ghost">
                        <Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" />
                      </Button>
                    </ConfirmAction>
                  </div>
                </div>

                {expanded && (
                  <div id={regionId}>
                    {view.tunnels.length ? (
                      <div role="table" aria-label={`${view.group.name} 分组的隧道`}>
                        <div
                          className="hidden grid-cols-[minmax(0,1fr)_130px_minmax(0,1.5fr)_minmax(0,1.5fr)_80px_84px] gap-4 border-b border-[color:var(--border-base)] px-6 xl:grid"
                          role="row"
                        >
                          {['隧道', '协议 / 状态', '监听 → 本地', '客户端', '限速', '操作'].map((label) => (
                            <div className={`txt-compact-xsmall-plus flex h-10 items-center text-[color:var(--fg-muted)] ${label === '操作' ? 'justify-end' : ''}`} key={label} role="columnheader">
                              {label}
                            </div>
                          ))}
                        </div>
                        <div role="rowgroup">
                          {view.tunnels.map(({ config: tunnel, owner }) => (
                            <TunnelRow
                              key={tunnel.name}
                              owner={owner}
                              tunnel={tunnel}
                              onDelete={onDeleteTunnel}
                              onDisconnect={onDisconnectClient}
                              onEdit={onEditTunnel}
                            />
                          ))}
                        </div>
                      </div>
                    ) : (
                      <div className="flex flex-col items-start gap-3 px-5 py-5 sm:flex-row sm:items-center sm:justify-between sm:px-6">
                        <p className="txt-compact-small text-[color:var(--fg-muted)]">
                          {view.totalTunnelCount ? '该分组没有符合筛选条件的隧道。' : '该分组还没有隧道。'}
                        </p>
                        <Button onClick={() => onCreateTunnel(view.group.name)} size="small" variant="secondary">
                          <Plus className="h-3.5 w-3.5" />
                          添加隧道
                        </Button>
                      </div>
                    )}
                  </div>
                )}
              </section>
            );
          }) : (
            <EmptyState description="请调整搜索词或筛选条件。" icon={Search} title="没有匹配的分组或隧道" />
          )}
        </>
      ) : (
        <EmptyState description="创建分组后即可在组内添加隧道。" icon={KeyRound} title="还没有访问分组" />
      )}
    </Panel>
  );
}

interface TunnelRowProps {
  owner?: TunnelRuntimeClient;
  tunnel: TunnelConfig;
  onDelete: (name: string) => void | Promise<void>;
  onDisconnect: (tunnel: TunnelConfig, client: TunnelRuntimeClient) => void | Promise<void>;
  onEdit: (tunnel: TunnelConfig) => void;
}

function TunnelRow({ owner, tunnel, onDelete, onDisconnect, onEdit }: TunnelRowProps) {
  const connectedAt = owner ? formatConnectedAt(owner.connected_at) : '';

  return (
    <div
      className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-3 border-b border-[color:var(--border-base)] px-5 py-3 last:border-b-0 hover:bg-[var(--bg-subtle)] xl:grid-cols-[minmax(0,1fr)_130px_minmax(0,1.5fr)_minmax(0,1.5fr)_80px_84px] xl:items-center xl:gap-x-4 xl:px-6"
      role="row"
    >
      <div className="col-start-1 row-start-1 min-w-0" role="cell">
        <strong className="txt-compact-small-plus block truncate font-medium text-[color:var(--fg-base)]">{tunnel.name}</strong>
      </div>

      <div className="col-span-2 row-start-2 flex flex-wrap items-center gap-1.5 xl:col-span-1 xl:col-start-2 xl:row-start-1" role="cell">
        <Badge>{kindLabel[tunnel.kind]}</Badge>
        <Badge tone={owner ? 'green' : 'neutral'}>{owner ? '在线' : '未连接'}</Badge>
      </div>

      <div className="col-span-2 row-start-3 min-w-0 xl:col-span-1 xl:col-start-3 xl:row-start-1" role="cell">
        <span className="txt-compact-xsmall mb-0.5 block text-[color:var(--fg-muted)] xl:hidden">监听 → 本地</span>
        <code className="txt-compact-xsmall block truncate text-[color:var(--fg-subtle)]" title={tunnelMapping(tunnel)}>
          {tunnelMapping(tunnel)}
        </code>
      </div>

      <div className="col-start-1 row-start-4 min-w-0 xl:col-start-4 xl:row-start-1" role="cell">
        <span className="txt-compact-xsmall mb-0.5 block text-[color:var(--fg-muted)] xl:hidden">客户端</span>
        {owner ? (
          <>
            <span className="txt-compact-small-plus block truncate text-[color:var(--fg-base)]">{owner.device_id}</span>
            <span className="txt-compact-xsmall block truncate text-[color:var(--fg-muted)]" title={`${owner.remote_address} · ${connectedAt}`}>
              {owner.remote_address} · {connectedAt}
            </span>
          </>
        ) : (
          <span className="txt-compact-small text-[color:var(--fg-muted)]">—</span>
        )}
      </div>

      <div className="col-start-2 row-start-4 min-w-0 xl:col-start-5 xl:row-start-1" role="cell">
        <span className="txt-compact-xsmall mb-0.5 block text-[color:var(--fg-muted)] xl:hidden">限速</span>
        <span className="txt-compact-small whitespace-nowrap text-[color:var(--fg-subtle)]">{formatLimit(tunnel.limit_bps)}</span>
      </div>

      <div className="col-start-2 row-start-1 flex justify-end gap-1 xl:col-start-6" role="cell">
        {owner && (
          <ConfirmAction
            confirmLabel="下线"
            description={`将断开隧道 ${tunnel.name} 的现有转发，客户端控制连接和其他隧道保持在线。`}
            onConfirm={() => onDisconnect(tunnel, owner)}
            title={`下线隧道 ${tunnel.name}？`}
          >
            <Button aria-label={`下线隧道 ${tunnel.name}`} size="icon" title="下线隧道" variant="ghost">
              <LogOut className="h-4 w-4 text-[color:var(--tag-red-text)]" />
            </Button>
          </ConfirmAction>
        )}
        <Button aria-label={`编辑 ${tunnel.name}`} onClick={() => onEdit(tunnel)} size="icon" title="编辑隧道" variant="ghost">
          <Pencil className="h-4 w-4" />
        </Button>
        <ConfirmAction
          confirmLabel="删除"
          description={`删除后，隧道 ${tunnel.name} 将立即停止提供服务。`}
          onConfirm={() => onDelete(tunnel.name)}
          title={`删除隧道 ${tunnel.name}？`}
        >
          <Button aria-label={`删除 ${tunnel.name}`} size="icon" title="删除隧道" variant="ghost">
            <Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" />
          </Button>
        </ConfirmAction>
      </div>
    </div>
  );
}

function initialExpandedGroups(groups: GroupConfig[]): Set<string> {
  try {
    const stored = sessionStorage.getItem(expandedGroupsStorageKey);
    if (stored !== null) {
      const names: unknown = JSON.parse(stored);
      if (Array.isArray(names) && names.every((name) => typeof name === 'string')) {
        const currentNames = new Set(groups.map((group) => group.name));
        return new Set(names.filter((name) => currentNames.has(name)));
      }
    }
  } catch {
    // 存储内容无效时使用默认展开策略。
  }

  return groups.length <= 6 ? new Set(groups.map((group) => group.name)) : new Set();
}

function sameSet(left: Set<string>, right: Set<string>): boolean {
  return left.size === right.size && [...left].every((value) => right.has(value));
}

function tunnelMapping(tunnel: TunnelConfig): string {
  if (tunnel.kind === 'socks5') return `${tunnel.bind} → 动态目标`;
  return `${tunnel.bind} → ${tunnelLocalTarget(tunnel)}`;
}

function formatConnectedAt(timestamp: number): string {
  return new Date(timestamp * 1000).toLocaleString();
}

function formatLimit(limitBps: number | null): string {
  return limitBps ? `${(limitBps / bytesPerKilobyte).toLocaleString()} KB/s` : '不限';
}
