use std::{cmp::Reverse, collections::HashMap, sync::Arc};

use http::Uri;

use crate::{Result, RouteConfig, config::ProxyConfig};

#[derive(Clone, Debug)]
pub(crate) struct Route {
    pub(crate) name: Arc<str>,
    pub(crate) upstream: Uri,
    pub(crate) tls: bool,
}

#[derive(Debug)]
pub(crate) struct Router {
    exact: HashMap<String, Vec<RouteEntry>>,
    wildcard: Vec<(String, Vec<RouteEntry>)>,
}

#[derive(Debug)]
struct RouteEntry {
    path_prefix: String,
    route: Route,
}

impl Router {
    pub(crate) fn new(config: &ProxyConfig) -> Result<Self> {
        let mut exact: HashMap<String, Vec<RouteEntry>> = HashMap::new();
        let mut wildcard: HashMap<String, Vec<RouteEntry>> = HashMap::new();
        for config in &config.routes {
            let entry = route_entry(config)?;
            if let Some(suffix) = config.host.strip_prefix("*.") {
                wildcard.entry(suffix.into()).or_default().push(entry);
            } else {
                exact.entry(config.host.clone()).or_default().push(entry);
            }
        }
        for entries in exact.values_mut().chain(wildcard.values_mut()) {
            entries.sort_unstable_by(|left, right| {
                right.path_prefix.len().cmp(&left.path_prefix.len())
            });
        }
        let mut wildcard: Vec<_> = wildcard.into_iter().collect();
        wildcard.sort_unstable_by_key(|entry| Reverse(entry.0.len()));
        Ok(Self { exact, wildcard })
    }

    pub(crate) fn find(&self, host: &str, path: &str) -> Option<Route> {
        self.exact
            .get(host)
            .and_then(|entries| find_path(entries, path))
            .or_else(|| {
                self.wildcard
                    .iter()
                    .find(|(suffix, _)| wildcard_matches(host, suffix))
                    .and_then(|(_, entries)| find_path(entries, path))
            })
    }
}

fn route_entry(config: &RouteConfig) -> Result<RouteEntry> {
    Ok(RouteEntry {
        path_prefix: config.path_prefix.clone(),
        route: Route {
            name: Arc::from(config.name.as_str()),
            upstream: config.upstream.parse().map_err(|_| {
                crate::ProxyError::InvalidConfig(format!("上游 URI 无效: {}", config.upstream))
            })?,
            tls: config.certificate.is_some(),
        },
    })
}

fn find_path(entries: &[RouteEntry], path: &str) -> Option<Route> {
    entries
        .iter()
        .find(|entry| path_matches(path, &entry.path_prefix))
        .map(|entry| entry.route.clone())
}

fn path_matches(path: &str, prefix: &str) -> bool {
    path == prefix
        || path.strip_prefix(prefix).is_some_and(|rest| {
            prefix.ends_with('/') || rest.as_bytes().first().is_some_and(|byte| *byte == b'/')
        })
}

fn wildcard_matches(host: &str, suffix: &str) -> bool {
    host.len() > suffix.len()
        && host.ends_with(suffix)
        && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf};

    use super::*;
    use crate::ProxyListenerConfig;

    fn config(routes: Vec<RouteConfig>) -> ProxyConfig {
        ProxyConfig {
            proxy: ProxyListenerConfig {
                http_bind: SocketAddr::from(([127, 0, 0, 1], 80)),
                https_bind: SocketAddr::from(([127, 0, 0, 1], 443)),
                cache_dir: PathBuf::from("cache"),
                max_connections: 16,
            },
            certificates: vec![],
            routes,
        }
    }

    fn route(name: &str, host: &str, path_prefix: &str) -> RouteConfig {
        RouteConfig {
            name: name.into(),
            host: host.into(),
            path_prefix: path_prefix.into(),
            upstream: "http://127.0.0.1:3000".into(),
            certificate: None,
        }
    }

    #[test]
    fn exact_host_and_longest_path_take_precedence() {
        let router = Router::new(&config(vec![
            route("root", "example.com", "/"),
            route("api", "example.com", "/api"),
            route("wildcard", "*.example.com", "/"),
        ]))
        .unwrap();
        assert_eq!(
            router.find("example.com", "/api/v1").unwrap().name.as_ref(),
            "api"
        );
        assert_eq!(
            router.find("www.example.com", "/").unwrap().name.as_ref(),
            "wildcard"
        );
        assert!(router.find("example.com", "/apix").is_some());
        assert!(router.find("badexample.com", "/").is_none());
    }
}
