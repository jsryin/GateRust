import type { ClientServiceConfig } from './client-types';

export interface EditableService extends Omit<ClientServiceConfig, 'target'> {
  id: string;
  target: string;
}

export interface EditableClientConfig {
  key: string;
  server: {
    address: string;
    name: string;
    caCertificate: string;
  };
  services: EditableService[];
}

export type NoticeKind = 'success' | 'error';

export interface Notice {
  kind: NoticeKind;
  message: string;
}
