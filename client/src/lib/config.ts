import type { ClientConfig } from './client-types';
import type { EditableClientConfig } from './types';

function optional(value: string | null | undefined): string | null {
  const trimmed = value?.trim() ?? '';
  return trimmed || null;
}

export function normalizeConfig(config: ClientConfig): ClientConfig {
  return {
    key: config.key.trim(),
    server: {
      address: config.server.address.trim(),
      name: optional(config.server.name),
      ca_certificate: optional(config.server.ca_certificate)
    },
    services: config.services.map((service) => ({
      name: service.name.trim(),
      kind: service.kind,
      target: service.kind === 'socks5' ? null : optional(service.target)
    }))
  };
}

export function createEditableConfig(config: ClientConfig): {
  baseline: string;
  draft: EditableClientConfig;
} {
  const normalized = normalizeConfig(config);
  return {
    baseline: JSON.stringify(normalized),
    draft: {
      key: normalized.key,
      server: {
        address: normalized.server.address,
        name: normalized.server.name ?? '',
        caCertificate: normalized.server.ca_certificate ?? ''
      },
      services: normalized.services.map((service) => ({
        ...service,
        id: crypto.randomUUID(),
        target: service.target ?? ''
      }))
    }
  };
}

export function prepareConfig(draft: EditableClientConfig): ClientConfig {
  return normalizeConfig({
    key: draft.key,
    server: {
      address: draft.server.address,
      name: draft.server.name,
      ca_certificate: draft.server.caCertificate
    },
    // 本地 id 只用于稳定渲染服务行，不进入 Rust 配置协议。
    services: draft.services.map(({ name, kind, target }) => ({ name, kind, target }))
  });
}
