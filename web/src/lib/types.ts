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

export type CertificateIssuer = 'lets_encrypt' | 'google_trust_services';
export type AcmeChallenge = 'http-01' | 'tls-alpn-01' | 'cloudflare-dns-01';

export interface CertificateConfig {
  name: string;
  domains: string[];
  email: string;
  issuer: CertificateIssuer;
  challenge: AcmeChallenge;
  production: boolean;
  cloudflare_api_token: string | null;
  cloudflare_zone_id: string | null;
  google_eab_key_id: string | null;
  google_eab_hmac_key: string | null;
  dns_propagation_seconds: number;
}

export interface RouteConfig {
  name: string;
  host: string;
  path_prefix: string;
  upstream: string;
  certificate: string | null;
}

export interface ProxyListenerConfig {
  http_bind: string;
  https_bind: string;
  cache_dir: string;
  max_connections: number;
}

export interface ProxyConfig {
  proxy: ProxyListenerConfig;
  certificates: CertificateConfig[];
  routes: RouteConfig[];
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
    waiting_session_ids: number[];
  }[];
}
