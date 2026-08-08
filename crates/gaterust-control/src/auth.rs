use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use argon2::{
    Argon2, PasswordHash, PasswordHasher as _, PasswordVerifier as _, password_hash::SaltString,
};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};

use crate::{ControlError, Result, config::WebConfig};

const LOGIN_WINDOW: Duration = Duration::from_mins(1);
const MAX_LOGIN_FAILURES: u8 = 5;
const MAX_TRACKED_ADDRESSES: usize = 4_096;
const TOKEN_REFRESH_INTERVAL: Duration = Duration::from_hours(1);

#[derive(Clone)]
pub(crate) struct AuthService {
    inner: Arc<AuthInner>,
}

struct AuthInner {
    username: String,
    password_hash: String,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    token_ttl: Duration,
    password_workers: Semaphore,
    attempts: Mutex<HashMap<IpAddr, LoginAttempt>>,
}

struct LoginAttempt {
    window_started: Instant,
    failures: u8,
}

#[derive(Debug)]
pub(crate) enum LoginError {
    InvalidCredentials,
    RateLimited,
    Internal,
}

#[derive(Clone, Deserialize, Serialize)]
struct Claims {
    sub: String,
    iat: u64,
    exp: u64,
}

pub(crate) struct AuthToken {
    pub(crate) value: String,
    pub(crate) expires_at: u64,
}

#[derive(Clone)]
pub(crate) struct AuthenticatedSession {
    issued_at: u64,
}

impl AuthService {
    pub(crate) fn new(config: &WebConfig) -> Result<Self> {
        PasswordHash::new(&config.admin_password_hash).map_err(|_| {
            ControlError::InvalidConfig("admin_password_hash 不是有效的 Argon2 PHC 字符串".into())
        })?;
        Ok(Self {
            inner: Arc::new(AuthInner {
                username: config.admin_username.clone(),
                password_hash: config.admin_password_hash.clone(),
                encoding_key: EncodingKey::from_secret(config.jwt_secret.as_bytes()),
                decoding_key: DecodingKey::from_secret(config.jwt_secret.as_bytes()),
                token_ttl: Duration::from_secs(config.token_ttl_seconds),
                password_workers: Semaphore::new(2),
                attempts: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub(crate) async fn login(
        &self,
        address: IpAddr,
        username: String,
        password: String,
    ) -> std::result::Result<AuthToken, LoginError> {
        if !self.can_attempt(address).await {
            return Err(LoginError::RateLimited);
        }
        let permit = self
            .inner
            .password_workers
            .acquire()
            .await
            .map_err(|_| LoginError::Internal)?;
        let expected_hash = self.inner.password_hash.clone();
        let verified = tokio::task::spawn_blocking(move || {
            let Ok(hash) = PasswordHash::new(&expected_hash) else {
                return false;
            };
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        })
        .await
        .map_err(|_| LoginError::Internal)?;
        drop(permit);

        if !verified || username != self.inner.username {
            self.record_failure(address).await;
            return Err(LoginError::InvalidCredentials);
        }
        self.inner.attempts.lock().await.remove(&address);
        let now = unix_seconds().map_err(|_| LoginError::Internal)?;
        self.issue_token(now)
    }

    pub(crate) fn verify_token(&self, token: &str) -> Option<AuthenticatedSession> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["sub", "iat", "exp"]);
        decode::<Claims>(token, &self.inner.decoding_key, &validation)
            .ok()
            .filter(|data| data.claims.sub == self.inner.username)
            .map(|data| AuthenticatedSession {
                issued_at: data.claims.iat,
            })
    }

    pub(crate) fn refresh_token(
        &self,
        session: &AuthenticatedSession,
    ) -> std::result::Result<Option<AuthToken>, LoginError> {
        let now = unix_seconds().map_err(|_| LoginError::Internal)?;
        if now.saturating_sub(session.issued_at) < TOKEN_REFRESH_INTERVAL.as_secs() {
            return Ok(None);
        }
        self.issue_token(now).map(Some)
    }

    fn issue_token(&self, issued_at: u64) -> std::result::Result<AuthToken, LoginError> {
        let expires_at = issued_at.saturating_add(self.inner.token_ttl.as_secs());
        let value = encode(
            &Header::new(Algorithm::HS256),
            &Claims {
                sub: self.inner.username.clone(),
                iat: issued_at,
                exp: expires_at,
            },
            &self.inner.encoding_key,
        )
        .map_err(|_| LoginError::Internal)?;
        Ok(AuthToken { value, expires_at })
    }

    async fn can_attempt(&self, address: IpAddr) -> bool {
        let mut attempts = self.inner.attempts.lock().await;
        let now = Instant::now();
        attempts.retain(|_, attempt| now.duration_since(attempt.window_started) < LOGIN_WINDOW);
        attempts
            .get(&address)
            .is_none_or(|attempt| attempt.failures < MAX_LOGIN_FAILURES)
    }

    async fn record_failure(&self, address: IpAddr) {
        let mut attempts = self.inner.attempts.lock().await;
        if attempts.len() >= MAX_TRACKED_ADDRESSES
            && !attempts.contains_key(&address)
            && let Some(oldest) = attempts
                .iter()
                .min_by_key(|(_, attempt)| attempt.window_started)
                .map(|(address, _)| *address)
        {
            attempts.remove(&oldest);
        }
        let attempt = attempts.entry(address).or_insert(LoginAttempt {
            window_started: Instant::now(),
            failures: 0,
        });
        attempt.failures = attempt.failures.saturating_add(1);
    }
}

/// 为 Web 管理员密码生成 Argon2id PHC 字符串。
///
/// # Errors
///
/// 密码为空或哈希失败时返回错误。
pub fn hash_password(password: &[u8]) -> Result<String> {
    if password.is_empty() {
        return Err(ControlError::InvalidConfig("管理员密码不能为空".into()));
    }
    let mut salt_bytes = [0_u8; 16];
    rand::rng().fill(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| ControlError::InvalidConfig(format!("生成密码盐失败: {error}")))?;
    Argon2::default()
        .hash_password(password, &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| ControlError::InvalidConfig(format!("生成密码哈希失败: {error}")))
}

fn unix_seconds() -> std::result::Result<u64, std::time::SystemTimeError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    fn config(password_hash: String) -> WebConfig {
        WebConfig {
            bind: "127.0.0.1:8080".parse().expect("测试地址有效"),
            static_dir: None,
            admin_username: "admin".into(),
            admin_password_hash: password_hash,
            jwt_secret: "0123456789abcdef0123456789abcdef".into(),
            token_ttl_seconds: 72 * 60 * 60,
            allowed_origins: Vec::new(),
        }
    }

    #[tokio::test]
    async fn login_uses_argon2_and_returns_valid_token() {
        let auth = AuthService::new(&config(hash_password(b"correct").expect("生成测试哈希")))
            .expect("创建认证服务");
        let address = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let issued_after = unix_seconds().expect("获取登录前时间");
        let token = auth
            .login(address, "admin".into(), "correct".into())
            .await
            .expect("正确凭据应登录成功");
        assert!(auth.verify_token(&token.value).is_some());
        assert!(token.expires_at >= issued_after + 72 * 60 * 60);
        assert!(matches!(
            auth.login(address, "admin".into(), "wrong".into()).await,
            Err(LoginError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn repeated_failures_are_rate_limited() {
        let auth = AuthService::new(&config(hash_password(b"correct").expect("生成测试哈希")))
            .expect("创建认证服务");
        let address = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..MAX_LOGIN_FAILURES {
            let _ = auth.login(address, "admin".into(), "wrong".into()).await;
        }
        assert!(matches!(
            auth.login(address, "admin".into(), "correct".into()).await,
            Err(LoginError::RateLimited)
        ));
    }

    #[test]
    fn active_session_is_refreshed_at_most_once_per_hour() {
        let auth = AuthService::new(&config(hash_password(b"correct").expect("生成测试哈希")))
            .expect("创建认证服务");
        let now = unix_seconds().expect("获取当前时间");
        let token = auth.issue_token(now).expect("签发测试令牌");
        let session = auth.verify_token(&token.value).expect("测试令牌有效");

        assert!(
            auth.refresh_token(&session)
                .expect("检查令牌续期")
                .is_none()
        );

        let refreshed = auth
            .issue_token(now - TOKEN_REFRESH_INTERVAL.as_secs())
            .and_then(|old_token| {
                let old_session = auth
                    .verify_token(&old_token.value)
                    .expect("旧测试令牌仍有效");
                auth.refresh_token(&old_session)
            })
            .expect("刷新令牌")
            .expect("签发新令牌");
        assert!(refreshed.expires_at >= now + 72 * 60 * 60);
        let refreshed_session = auth
            .verify_token(&refreshed.value)
            .expect("刷新后的令牌有效");
        assert!(
            auth.refresh_token(&refreshed_session)
                .expect("检查刷新后的令牌")
                .is_none()
        );
    }
}
