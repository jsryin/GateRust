import { Plus, Trash2 } from 'lucide-react';
import { memo, useCallback, useLayoutEffect, useRef } from 'react';
import type { TunnelKind } from '../lib/client-types';
import type { EditableService } from '../lib/types';

type ServicesUpdater = (update: (services: EditableService[]) => EditableService[]) => void;
type ServicePatch = Partial<Pick<EditableService, 'kind' | 'name' | 'target'>>;

interface ServiceListProps {
  onChange: ServicesUpdater;
  services: EditableService[];
}

interface ServiceRowProps {
  onRemove: (id: string) => void;
  onUpdate: (id: string, patch: ServicePatch) => void;
  service: EditableService;
}

const ServiceRow = memo(function ServiceRow({ onRemove, onUpdate, service }: ServiceRowProps) {
  const targetDisabled = service.kind === 'socks5';

  return (
    <div className="service-row">
      <label>
        <span className="mobile-label">名称</span>
        <input
          data-service-id={service.id}
          maxLength={64}
          onChange={(event) => onUpdate(service.id, { name: event.target.value })}
          pattern="[A-Za-z0-9_-]+"
          placeholder="ssh"
          required
          spellCheck={false}
          value={service.name}
        />
      </label>
      <label>
        <span className="mobile-label">协议</span>
        <select
          aria-label="服务协议"
          onChange={(event) => onUpdate(service.id, { kind: event.target.value as TunnelKind })}
          value={service.kind}
        >
          <option value="tcp">TCP</option>
          <option value="udp">UDP</option>
          <option value="socks5">SOCKS5</option>
        </select>
      </label>
      <label className={targetDisabled ? 'disabled-field' : undefined}>
        <span className="mobile-label">目标地址</span>
        <input
          disabled={targetDisabled}
          onChange={(event) => onUpdate(service.id, { target: event.target.value })}
          placeholder={service.kind === 'udp' ? '127.0.0.1:27015' : '127.0.0.1:22'}
          required={!targetDisabled}
          spellCheck={false}
          value={service.target}
        />
      </label>
      <button
        aria-label={`删除服务 ${service.name || '未命名'}`}
        className="icon-button danger-button"
        onClick={() => onRemove(service.id)}
        title="删除服务"
        type="button"
      >
        <Trash2 aria-hidden="true" size={17} strokeWidth={1.8} />
      </button>
    </div>
  );
});

export function ServiceList({ onChange, services }: ServiceListProps) {
  const focusServiceId = useRef<string | null>(null);
  const listElement = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    if (!focusServiceId.current) return;
    const input = listElement.current?.querySelector<HTMLInputElement>(
      `[data-service-id="${focusServiceId.current}"]`
    );
    input?.focus();
    focusServiceId.current = null;
  }, [services]);

  const addService = useCallback(() => {
    const id = crypto.randomUUID();
    focusServiceId.current = id;
    onChange((current) => [
      ...current,
      { id, name: '', kind: 'tcp', target: '127.0.0.1:' }
    ]);
  }, [onChange]);

  const removeService = useCallback(
    (id: string) => onChange((current) => current.filter((service) => service.id !== id)),
    [onChange]
  );

  const updateService = useCallback(
    (id: string, patch: ServicePatch) =>
      onChange((current) =>
        current.map((service) => (service.id === id ? { ...service, ...patch } : service))
      ),
    [onChange]
  );

  return (
    <section aria-labelledby="services-heading" className="settings-section services-section">
      <header className="section-heading">
        <div>
          <h2 id="services-heading">本地服务</h2>
          <span>{services.length} 项</span>
        </div>
        <button
          className="secondary-button"
          disabled={services.length >= 256}
          onClick={addService}
          type="button"
        >
          <Plus aria-hidden="true" size={16} strokeWidth={2} />
          添加服务
        </button>
      </header>

      {services.length > 0 ? (
        <div className="service-table">
          <div aria-hidden="true" className="service-table-head">
            <span>名称</span>
            <span>协议</span>
            <span>目标地址</span>
            <span />
          </div>
          <div className="service-list" ref={listElement}>
            {services.map((service) => (
              <ServiceRow
                key={service.id}
                onRemove={removeService}
                onUpdate={updateService}
                service={service}
              />
            ))}
          </div>
        </div>
      ) : (
        <div className="empty-state">
          <span className="empty-icon"><Plus aria-hidden="true" size={18} /></span>
          <strong>暂无本地服务</strong>
        </div>
      )}
    </section>
  );
}
