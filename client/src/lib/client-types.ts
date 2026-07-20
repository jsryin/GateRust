export type TunnelKind = 'tcp' | 'udp' | 'socks5';

export interface ClientServerConfig {
  address: string;
  name?: string | null;
  ca_certificate?: string | null;
}

export interface ClientServiceConfig {
  name: string;
  kind: TunnelKind;
  target?: string | null;
}

export interface ClientConfig {
  key: string;
  server: ClientServerConfig;
  services: ClientServiceConfig[];
}

export interface ConfigResponse {
  path: string;
  config: ClientConfig;
}

export type ClientStatusState =
  | 'starting'
  | 'unconfigured'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'stopped'
  | 'offline';

export interface ClientStatus {
  state: ClientStatusState;
  message: string | null;
  server: string | null;
  device_id: string | null;
  retry_seconds: number | null;
}
