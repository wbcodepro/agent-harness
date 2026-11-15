//! Password hashing (Argon2id) and password rules.

use crate::config::Argon2Params;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};
use argon2::{Algorithm, Argon2, Params, Version};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Minimum and maximum password length in characters.
pub const MIN_LEN: usize = 10;
pub const MAX_LEN: usize = 128;

/// Common passwords that pass the length rule but must still be refused.
const COMMON: &[&str] = &[
    "1234567890",
    "12345678910",
    "0123456789",
    "qwertyuiop",
    "password12",
    "password123",
    "password1234",
    "passwordpassword",
    "iloveyou123",
    "qwerty1234",
    "qwerty12345",
    "1q2w3e4r5t",
    "1qaz2wsx3edc",
    "abcdefghij",
    "letmein123",
    "welcome123",
    "administrator",
    "changeme123",
    "football123",
    "baseball123",
    "trustno1234",
    "sunshine123",
    "princess123",
    "dragon12345",
    "monkey12345",
];

/// Why a password was refused.
pub fn check_strength(password: &str, email: &str, name: &str) -> Result<(), String> {
    let len = password.chars().count();
    if len < MIN_LEN {
        return Err(format!("password must have at least {MIN_LEN} characters"));
    }
    if len > MAX_LEN {
        return Err(format!("password must have at most {MAX_LEN} characters"));
    }
    let mut distinct: Vec<char> = password.chars().collect();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() < 5 {
        return Err("password must use at least 5 different characters".into());
    }
    let lower = password.to_lowercase();
    if COMMON.contains(&lower.as_str()) {
        return Err("password is too common".into());
    }
    let local = email.split('@').next().unwrap_or("").to_lowercase();
    if local.len() >= 4 && lower.contains(&local) {
        return Err("password must not contain your email address".into());
    }
    let name = name.trim().to_lowercase();
    if name.chars().count() >= 4 && lower.contains(&name) {
        return Err("password must not contain your name".into());
    }
    Ok(())
}

/// Why hashing did not happen.
#[derive(Debug, thiserror::Error)]
pub enum HashError {
    /// No hashing slot became free in time.
    #[error("password hashing is busy")]
    Busy,
    #[error("password hashing failed: {0}")]
    Failed(String),
}

impl From<HashError> for crate::error::ApiError {
    fn from(e: HashError) -> Self {
        match e {
            HashError::Busy => crate::error::ApiError::busy(),
            HashError::Failed(m) => crate::error::ApiError::internal(m),
        }
    }
}

/// Hashes and verifies passwords on blocking threads, at most
/// `concurrency` at a time (each hash uses `m_kib` of memory). A request
/// that waits longer than `queue_timeout` for a slot gets [`HashError::Busy`].
#[derive(Clone)]
pub struct PasswordHasher {
    params: Argon2Params,
    permits: Arc<Semaphore>,
    queue_timeout: Duration,
    /// A hash of a random password, verified against when an account does
    /// not exist so that the response time does not reveal it.
    dummy: Arc<String>,
}

impl PasswordHasher {
    pub async fn new(
        params: Argon2Params,
        concurrency: usize,
        queue_timeout: Duration,
    ) -> Result<Self, String> {
        let random = crate::util::b64url(&crate::util::random_bytes::<24>());
        let dummy = tokio::task::spawn_blocking(move || hash_sync(params, &random))
            .await
            .map_err(|e| e.to_string())??;
        Ok(Self {
            params,
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
            queue_timeout,
            dummy: Arc::new(dummy),
        })
    }

    async fn permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, HashError> {
        match tokio::time::timeout(self.queue_timeout, self.permits.acquire()).await {
            Ok(Ok(p)) => Ok(p),
            Ok(Err(e)) => Err(HashError::Failed(e.to_string())),
            Err(_) => Err(HashError::Busy),
        }
    }

    /// The PHC string for `password`.
    pub async fn hash(&self, password: &str) -> Result<String, HashError> {
        let _permit = self.permit().await?;
        let params = self.params;
        let password = password.to_string();
        tokio::task::spawn_blocking(move || hash_sync(params, &password))
            .await
            .map_err(|e| HashError::Failed(e.to_string()))?
            .map_err(HashError::Failed)
    }

    /// Whether `password` matches `phc`. With `None`, verifies against a
    /// dummy hash and returns false (constant work for unknown accounts).
    pub async fn verify(&self, password: &str, phc: Option<&str>) -> Result<bool, HashError> {
        let _permit = self.permit().await?;
        let known = phc.is_some();
        let phc = phc
            .map(str::to_string)
            .unwrap_or_else(|| (*self.dummy).clone());
        let password = password.to_string();
        let ok = tokio::task::spawn_blocking(move || verify_sync(&password, &phc))
            .await
            .unwrap_or(false);
        Ok(ok && known)
    }
}

fn argon(params: Argon2Params) -> Result<Argon2<'static>, String> {
    let p = Params::new(params.m_kib, params.t, params.p, None).map_err(|e| e.to_string())?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, p))
}

fn hash_sync(params: Argon2Params, password: &str) -> Result<String, String> {
    argon(params)?
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

fn verify_sync(password: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    // The parameters come from the stored hash.
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_rules() {
        assert!(check_strength("short", "a@b.co", "Ann").is_err());
        assert!(check_strength("aaaaaaaaaaaa", "a@b.co", "Ann").is_err());
        assert!(check_strength("password123", "a@b.co", "Ann").is_err());
        assert!(check_strength("alice-is-great", "alice@b.co", "Ann").is_err());
        assert!(check_strength("xx-robert-xx-9", "z@b.co", "Robert").is_err());
        assert!(check_strength(&"ab1-".repeat(40), "z@b.co", "Bo").is_err());
        assert!(check_strength("correct horse battery", "z@b.co", "Bo").is_ok());
    }

    #[tokio::test]
    async fn production_parameters_hash_and_verify() {
        let h = PasswordHasher::new(Argon2Params::default(), 1, Duration::from_secs(60))
            .await
            .unwrap();
        let phc = h.hash("correct horse battery").await.unwrap();
        assert!(phc.starts_with("$argon2id$v=19$m=65536,t=3,p=1$"), "{phc}");
        assert!(h.verify("correct horse battery", Some(&phc)).await.unwrap());
        assert!(!h.verify("wrong horse battery", Some(&phc)).await.unwrap());
        assert!(!h.verify("correct horse battery", None).await.unwrap());
    }

    #[tokio::test]
    async fn a_full_queue_answers_busy() {
        let params = Argon2Params {
            m_kib: 1024,
            t: 1,
            p: 1,
        };
        let h = PasswordHasher::new(params, 1, Duration::from_millis(50))
            .await
            .unwrap();
        let held = h.permits.clone().acquire_owned().await.unwrap();
        assert!(matches!(h.hash("x").await, Err(HashError::Busy)));
        assert!(matches!(h.verify("x", None).await, Err(HashError::Busy)));
        drop(held);
        assert!(h.hash("x").await.is_ok());
    }
}
