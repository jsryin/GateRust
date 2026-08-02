export type TunnelKind = 'tcp' | 'udp' | 'socks5';

export interface GroupConfig {
  name: string;
  key: string;
}

export interface TunnelConfig {
  name: string;
  group: string;
  kind: TunnelKind;
  bind: string;
  local_port: number | null;
  limit_bps: number | null;
  max_connections: number;
  max_udp_sessions: number;
  udp_idle_seconds: number;
}

export interface ServerQuicConfig {
  bind: string;
  certificate: string;
  private_key: string;
}

export interface ServerConfig {
  quic: ServerQuicConfig;
  groups: GroupConfig[];
  tunnels: TunnelConfig[];
}

export type AcmeProvider = 'lets_encrypt' | 'google_cloud';
export type AcmeEnvironment = 'production' | 'staging';
export type KeyAlgorithm = 'ec256' | 'rsa2048';
export type DnsProvider = 'cloudflare' | 'go_daddy' | 'aliyun' | 'tencent_cloud';

export interface AcmeAccountView {
  id: string;
  name: string;
  provider: AcmeProvider;
  environment: AcmeEnvironment;
  email: string;
  key_algorithm: KeyAlgorithm;
  eab_key_id: string | null;
  eab_hmac_key_configured: boolean;
}

export interface AcmeAccountInput {
  id: string;
  name: string;
  provider: AcmeProvider;
  environment: AcmeEnvironment;
  email: string;
  key_algorithm: KeyAlgorithm;
  eab_key_id: string | null;
  eab_hmac_key: string | null;
}

export interface DnsAccountView {
  id: string;
  name: string;
  provider: DnsProvider;
  api_token_configured: boolean;
  access_key_configured: boolean;
  secret_key_configured: boolean;
}

export interface DnsAccountInput {
  id: string;
  name: string;
  provider: DnsProvider;
  api_token: string | null;
  access_key: string | null;
  secret_key: string | null;
}

export type CertificateValidation =
  | { method: 'dns_account'; dns_account_id: string }
  | { method: 'manual' };

export interface CertificateConfig {
  id: string;
  name: string;
  domains: string[];
  acme_account_id: string;
  validation: CertificateValidation | null;
  auto_renew: boolean;
  migration_error?: string | null;
}

export interface RouteConfig {
  name: string;
  host: string;
  path_prefix: string;
  upstream: string;
  certificate_id: string | null;
}

export interface ProxyListenerConfig {
  http_bind: string;
  https_bind: string;
  cache_dir: string;
  max_connections: number;
}

export interface ProxyConfig {
  proxy: ProxyListenerConfig;
  acme_accounts: AcmeAccountView[];
  dns_accounts: DnsAccountView[];
  certificates: CertificateConfig[];
  routes: RouteConfig[];
}

export type CertificateStatus = 'idle' | 'issuing' | 'waiting_dns' | 'valid' | 'renewing' | 'failed' | 'expired';

export interface ManualDnsRecord {
  name: string;
  value: string;
}

export interface CertificateRuntimeStatus {
  certificate_id: string;
  status: CertificateStatus;
  expires_at: number | null;
  last_error: string | null;
  manual_records: ManualDnsRecord[];
}

export interface ProxyRuntimeState {
  certificates: CertificateRuntimeStatus[];
  config_status: {
    revision: number;
    last_apply_error: string | null;
  };
}

export interface ConfigSnapshot {
  tunnel?: ServerConfig | null;
  proxy?: ProxyConfig | null;
}

export interface Dashboard {
  revision: number;
  tunnel_enabled: boolean;
  proxy_enabled: boolean;
  groups: number;
  tunnels: number;
  certificates: number;
  routes: number;
}

export interface ClientService {
  name: string;
  kind: TunnelKind;
  target: string | null;
}

export interface TunnelRuntimeClient {
  session_id: number;
  device_id: string;
  group: string;
  remote_address: string;
  connected_at: number;
}

export interface TunnelRuntimeState {
  clients: TunnelRuntimeClient[];
  tunnels: {
    name: string;
    owner_session_id: number | null;
  }[];
  config_status: {
    revision: number;
    last_apply_error: string | null;
  };
}
