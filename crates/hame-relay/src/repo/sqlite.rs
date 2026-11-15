//! SQLite implementation of [`Repository`] (WAL mode).

use super::*;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
    SqliteSynchronous,
};
use sqlx::{Row, Sqlite, Transaction};
use std::path::Path;
use std::time::Duration;

/// The SQLite repository.
#[derive(Clone)]
pub struct SqliteRepo {
    pool: SqlitePool,
}

fn db(e: sqlx::Error) -> RepoError {
    if let sqlx::Error::Database(d) = &e
        && d.is_unique_violation()
    {
        return RepoError::Conflict;
    }
    RepoError::Db(e.to_string())
}

impl SqliteRepo {
    /// Opens (creating if needed) the database file and runs migrations.
    pub async fn open(path: &Path) -> RepoResult<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .map_err(db)?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| RepoError::Db(e.to_string()))?;
        Ok(Self { pool })
    }

    async fn begin(&self) -> RepoResult<Transaction<'static, Sqlite>> {
        self.pool.begin().await.map_err(db)
    }
}

fn account_row(r: &SqliteRow) -> Account {
    Account {
        id: r.get("id"),
        name: r.get("name"),
        email: r.get("email"),
        email_verified: r.get::<i64, _>("email_verified") != 0,
        password_hash: r.get("password_hash"),
        created_at: r.get("created_at"),
    }
}

fn device_row(r: &SqliteRow) -> Device {
    Device {
        id: r.get("id"),
        account_id: r.get("account_id"),
        name: r.get("name"),
        kind: r.get("kind"),
        ed25519: r.get("ed25519"),
        x25519: r.get("x25519"),
        created_at: r.get("created_at"),
        last_seen_at: r.get("last_seen_at"),
        revoked_at: r.get("revoked_at"),
        last_seq: r.get("last_seq"),
        acked_seq: r.get("acked_seq"),
        push_provider: r.get("push_provider"),
        push_token: r.get("push_token"),
    }
}

fn passkey_row(r: &SqliteRow) -> StoredPasskey {
    StoredPasskey {
        id: r.get("id"),
        account_id: r.get("account_id"),
        credential_id: r.get("credential_id"),
        name: r.get("name"),
        data: r.get("data"),
        created_at: r.get("created_at"),
        last_used_at: r.get("last_used_at"),
    }
}

fn device_code_row(r: &SqliteRow) -> DeviceCodeRow {
    DeviceCodeRow {
        device_code_hash: r.get("device_code_hash"),
        user_code_hash: r.get("user_code_hash"),
        client_name: r.get("client_name"),
        status: r.get("status"),
        account_id: r.get("account_id"),
        interval_secs: r.get("interval_secs"),
        last_poll_at: r.get("last_poll_at"),
        expires_at: r.get("expires_at"),
        created_at: r.get("created_at"),
    }
}

const DEVICE_COLS: &str = "id, account_id, name, kind, ed25519, x25519, created_at, last_seen_at, \
     revoked_at, last_seq, acked_seq, push_provider, push_token";

#[async_trait]
impl Repository for SqliteRepo {
    async fn create_account(&self, a: &Account) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO accounts (id, name, email, email_verified, password_hash, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&a.id)
        .bind(&a.name)
        .bind(&a.email)
        .bind(a.email_verified as i64)
        .bind(&a.password_hash)
        .bind(a.created_at)
        .bind(a.created_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn account(&self, id: &str) -> RepoResult<Option<Account>> {
        let row = sqlx::query("SELECT * FROM accounts WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.as_ref().map(account_row))
    }

    async fn account_by_email(&self, email: &str) -> RepoResult<Option<Account>> {
        let row = sqlx::query("SELECT * FROM accounts WHERE email = ?")
            .bind(email)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.as_ref().map(account_row))
    }

    async fn set_email_verified(&self, id: &str, now: i64) -> RepoResult<()> {
        sqlx::query("UPDATE accounts SET email_verified = 1, updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn set_password(&self, id: &str, hash: Option<&str>, now: i64) -> RepoResult<()> {
        sqlx::query("UPDATE accounts SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(hash)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn set_name(&self, id: &str, name: &str, now: i64) -> RepoResult<()> {
        sqlx::query("UPDATE accounts SET name = ?, updated_at = ? WHERE id = ?")
            .bind(name)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn insert_email_token(
        &self,
        hash: &str,
        account_id: &str,
        purpose: &str,
        expires_at: i64,
        now: i64,
    ) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO email_tokens (token_hash, account_id, purpose, expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(hash)
        .bind(account_id)
        .bind(purpose)
        .bind(expires_at)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn consume_email_token(
        &self,
        hash: &str,
        purpose: &str,
        now: i64,
    ) -> RepoResult<Option<String>> {
        let row = sqlx::query(
            "UPDATE email_tokens SET used_at = ? \
             WHERE token_hash = ? AND purpose = ? AND used_at IS NULL AND expires_at > ? \
             RETURNING account_id",
        )
        .bind(now)
        .bind(hash)
        .bind(purpose)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| r.get("account_id")))
    }

    async fn peek_email_token(
        &self,
        hash: &str,
        purpose: &str,
        now: i64,
    ) -> RepoResult<Option<String>> {
        let row = sqlx::query(
            "SELECT account_id FROM email_tokens \
             WHERE token_hash = ? AND purpose = ? AND used_at IS NULL AND expires_at > ?",
        )
        .bind(hash)
        .bind(purpose)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| r.get("account_id")))
    }

    async fn delete_email_tokens(&self, account_id: &str, purpose: &str) -> RepoResult<()> {
        sqlx::query("DELETE FROM email_tokens WHERE account_id = ? AND purpose = ?")
            .bind(account_id)
            .bind(purpose)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn identity_account(&self, provider: &str, subject: &str) -> RepoResult<Option<String>> {
        let row =
            sqlx::query("SELECT account_id FROM identities WHERE provider = ? AND subject = ?")
                .bind(provider)
                .bind(subject)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(row.map(|r| r.get("account_id")))
    }

    async fn link_identity(
        &self,
        provider: &str,
        subject: &str,
        account_id: &str,
        email: Option<&str>,
        now: i64,
    ) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO identities (provider, subject, account_id, email, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(provider)
        .bind(subject)
        .bind(account_id)
        .bind(email)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn identities(&self, account_id: &str) -> RepoResult<Vec<Identity>> {
        let rows = sqlx::query(
            "SELECT provider, email, created_at FROM identities WHERE account_id = ? ORDER BY created_at",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .iter()
            .map(|r| Identity {
                provider: r.get("provider"),
                email: r.get("email"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    async fn create_session(
        &self,
        id: &str,
        account_id: &str,
        device_id: Option<&str>,
        method: &str,
        now: i64,
    ) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO sessions (id, account_id, device_id, method, created_at, last_used_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(account_id)
        .bind(device_id)
        .bind(method)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_access_token(
        &self,
        hash: &str,
        session_id: &str,
        expires_at: i64,
    ) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO access_tokens (token_hash, session_id, expires_at) VALUES (?, ?, ?)",
        )
        .bind(hash)
        .bind(session_id)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn insert_refresh_token(
        &self,
        hash: &str,
        session_id: &str,
        expires_at: i64,
        now: i64,
    ) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO refresh_tokens (token_hash, session_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(hash)
        .bind(session_id)
        .bind(expires_at)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn authenticate(&self, access_hash: &str, now: i64) -> RepoResult<Option<AuthContext>> {
        let row = sqlx::query(
            "SELECT s.id AS session_id, s.account_id, s.device_id \
             FROM access_tokens t JOIN sessions s ON s.id = t.session_id \
             LEFT JOIN devices d ON d.id = s.device_id \
             WHERE t.token_hash = ? AND t.expires_at > ? AND s.revoked_at IS NULL \
               AND d.revoked_at IS NULL",
        )
        .bind(access_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| AuthContext {
            session_id: r.get("session_id"),
            account_id: r.get("account_id"),
            device_id: r.get("device_id"),
        }))
    }

    async fn rotate_refresh_token(
        &self,
        old_hash: &str,
        new_hash: &str,
        new_expires_at: i64,
        now: i64,
    ) -> RepoResult<RefreshOutcome> {
        let mut tx = self.begin().await?;
        let row = sqlx::query(
            "SELECT t.used_at, t.expires_at, s.id AS session_id, s.account_id, s.device_id, s.revoked_at \
             FROM refresh_tokens t JOIN sessions s ON s.id = t.session_id WHERE t.token_hash = ?",
        )
        .bind(old_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?;
        let Some(row) = row else {
            return Ok(RefreshOutcome::Invalid);
        };
        let ctx = AuthContext {
            session_id: row.get("session_id"),
            account_id: row.get("account_id"),
            device_id: row.get("device_id"),
        };
        if row.get::<Option<i64>, _>("revoked_at").is_some() {
            return Ok(RefreshOutcome::Invalid);
        }
        if row.get::<Option<i64>, _>("used_at").is_some() {
            sqlx::query(
                "UPDATE sessions SET revoked_at = ?, revoke_reason = 'refresh_token_reused' WHERE id = ?",
            )
            .bind(now)
            .bind(&ctx.session_id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            tx.commit().await.map_err(db)?;
            return Ok(RefreshOutcome::Reused(ctx));
        }
        if row.get::<i64, _>("expires_at") <= now {
            return Ok(RefreshOutcome::Invalid);
        }
        let used = sqlx::query(
            "UPDATE refresh_tokens SET used_at = ? WHERE token_hash = ? AND used_at IS NULL",
        )
        .bind(now)
        .bind(old_hash)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        if used.rows_affected() != 1 {
            return Ok(RefreshOutcome::Invalid);
        }
        sqlx::query(
            "INSERT INTO refresh_tokens (token_hash, session_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(new_hash)
        .bind(&ctx.session_id)
        .bind(new_expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query("UPDATE sessions SET last_used_at = ? WHERE id = ?")
            .bind(now)
            .bind(&ctx.session_id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(RefreshOutcome::Rotated(ctx))
    }

    async fn revoke_session(&self, id: &str, reason: &str, now: i64) -> RepoResult<Option<String>> {
        let row = sqlx::query(
            "UPDATE sessions SET revoked_at = ?, revoke_reason = ? \
             WHERE id = ? AND revoked_at IS NULL RETURNING device_id",
        )
        .bind(now)
        .bind(reason)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.and_then(|r| r.get("device_id")))
    }

    async fn revoke_account_sessions(
        &self,
        account_id: &str,
        except: Option<&str>,
        reason: &str,
        now: i64,
    ) -> RepoResult<Vec<String>> {
        let rows = sqlx::query(
            "UPDATE sessions SET revoked_at = ?, revoke_reason = ? \
             WHERE account_id = ? AND revoked_at IS NULL AND id IS NOT ? RETURNING device_id",
        )
        .bind(now)
        .bind(reason)
        .bind(account_id)
        .bind(except)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .iter()
            .filter_map(|r| r.get::<Option<String>, _>("device_id"))
            .collect())
    }

    async fn bind_session_device(&self, session_id: &str, device_id: &str) -> RepoResult<()> {
        sqlx::query("UPDATE sessions SET device_id = ? WHERE id = ?")
            .bind(device_id)
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn create_device(&self, d: &Device) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO devices (id, account_id, name, kind, ed25519, x25519, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&d.id)
        .bind(&d.account_id)
        .bind(&d.name)
        .bind(&d.kind)
        .bind(&d.ed25519)
        .bind(&d.x25519)
        .bind(d.created_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn device(&self, id: &str) -> RepoResult<Option<Device>> {
        let sql = format!("SELECT {DEVICE_COLS} FROM devices WHERE id = ?");
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.as_ref().map(device_row))
    }

    async fn devices(&self, account_id: &str) -> RepoResult<Vec<Device>> {
        let sql = format!(
            "SELECT {DEVICE_COLS} FROM devices WHERE account_id = ? AND revoked_at IS NULL ORDER BY created_at"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(account_id)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        Ok(rows.iter().map(device_row).collect())
    }

    async fn revoke_device(&self, id: &str, now: i64) -> RepoResult<()> {
        let mut tx = self.begin().await?;
        sqlx::query(
            "UPDATE devices SET revoked_at = ?, push_provider = NULL, push_token = NULL \
             WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query(
            "UPDATE sessions SET revoked_at = ?, revoke_reason = 'device_revoked' \
             WHERE device_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query("DELETE FROM mailbox WHERE recipient_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("DELETE FROM host_grants WHERE host_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn set_push_token(
        &self,
        id: &str,
        provider: Option<&str>,
        token: Option<&str>,
    ) -> RepoResult<()> {
        sqlx::query("UPDATE devices SET push_provider = ?, push_token = ? WHERE id = ?")
            .bind(provider)
            .bind(token)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn touch_device(&self, id: &str, now: i64) -> RepoResult<()> {
        sqlx::query("UPDATE devices SET last_seen_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn put_grants(
        &self,
        host_id: &str,
        document: &[u8],
        signature: &[u8],
        now: i64,
    ) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO host_grants (host_id, document, signature, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(host_id) DO UPDATE SET document = excluded.document, \
             signature = excluded.signature, updated_at = excluded.updated_at",
        )
        .bind(host_id)
        .bind(document)
        .bind(signature)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn grants(&self, host_id: &str) -> RepoResult<Option<Grants>> {
        let row = sqlx::query(
            "SELECT document, signature, updated_at FROM host_grants WHERE host_id = ?",
        )
        .bind(host_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| Grants {
            document: r.get("document"),
            signature: r.get("signature"),
            updated_at: r.get("updated_at"),
        }))
    }

    async fn enqueue(
        &self,
        recipient_id: &str,
        sender_id: &str,
        envelope: &[u8],
        now: i64,
        max_queued: i64,
    ) -> RepoResult<EnqueueOutcome> {
        let mut tx = self.begin().await?;
        let queued: i64 = sqlx::query("SELECT COUNT(*) AS n FROM mailbox WHERE recipient_id = ?")
            .bind(recipient_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?
            .get("n");
        if queued >= max_queued {
            return Ok(EnqueueOutcome::Full);
        }
        let seq: i64 = sqlx::query(
            "UPDATE devices SET last_seq = last_seq + 1 WHERE id = ? RETURNING last_seq",
        )
        .bind(recipient_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?
        .get("last_seq");
        sqlx::query(
            "INSERT INTO mailbox (recipient_id, seq, sender_id, envelope, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(recipient_id)
        .bind(seq)
        .bind(sender_id)
        .bind(envelope)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(EnqueueOutcome::Queued(seq))
    }

    async fn fetch_after(
        &self,
        recipient_id: &str,
        after: i64,
        limit: i64,
    ) -> RepoResult<Vec<MailItem>> {
        let rows = sqlx::query(
            "SELECT seq, sender_id, envelope FROM mailbox WHERE recipient_id = ? AND seq > ? \
             ORDER BY seq LIMIT ?",
        )
        .bind(recipient_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .iter()
            .map(|r| MailItem {
                seq: r.get("seq"),
                sender_id: r.get("sender_id"),
                envelope: r.get("envelope"),
            })
            .collect())
    }

    async fn ack(&self, recipient_id: &str, upto: i64) -> RepoResult<()> {
        let mut tx = self.begin().await?;
        sqlx::query("DELETE FROM mailbox WHERE recipient_id = ? AND seq <= ?")
            .bind(recipient_id)
            .bind(upto)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("UPDATE devices SET acked_seq = MAX(acked_seq, MIN(?, last_seq)) WHERE id = ?")
            .bind(upto)
            .bind(recipient_id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn insert_passkey(&self, p: &StoredPasskey) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO passkeys (id, account_id, credential_id, name, data, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&p.id)
        .bind(&p.account_id)
        .bind(&p.credential_id)
        .bind(&p.name)
        .bind(&p.data)
        .bind(p.created_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn passkeys(&self, account_id: &str) -> RepoResult<Vec<StoredPasskey>> {
        let rows = sqlx::query("SELECT * FROM passkeys WHERE account_id = ? ORDER BY created_at")
            .bind(account_id)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        Ok(rows.iter().map(passkey_row).collect())
    }

    async fn passkey_by_credential(
        &self,
        credential_id: &str,
    ) -> RepoResult<Option<StoredPasskey>> {
        let row = sqlx::query("SELECT * FROM passkeys WHERE credential_id = ?")
            .bind(credential_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.as_ref().map(passkey_row))
    }

    async fn update_passkey_data(&self, id: &str, data: &str, now: i64) -> RepoResult<()> {
        sqlx::query("UPDATE passkeys SET data = ?, last_used_at = ? WHERE id = ?")
            .bind(data)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn delete_passkey(&self, account_id: &str, id: &str) -> RepoResult<bool> {
        let r = sqlx::query("DELETE FROM passkeys WHERE account_id = ? AND id = ?")
            .bind(account_id)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(r.rows_affected() == 1)
    }

    async fn put_ephemeral(
        &self,
        kind: &str,
        key: &str,
        account_id: Option<&str>,
        data: &str,
        expires_at: i64,
    ) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO ephemeral (kind, key, account_id, data, expires_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(kind)
        .bind(key)
        .bind(account_id)
        .bind(data)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn take_ephemeral(
        &self,
        kind: &str,
        key: &str,
        now: i64,
    ) -> RepoResult<Option<(Option<String>, String)>> {
        let row = sqlx::query(
            "DELETE FROM ephemeral WHERE kind = ? AND key = ? RETURNING account_id, data, expires_at",
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row
            .filter(|r| r.get::<i64, _>("expires_at") > now)
            .map(|r| (r.get("account_id"), r.get("data"))))
    }

    async fn insert_device_code(&self, c: &DeviceCodeRow) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO device_codes (device_code_hash, user_code_hash, client_name, status, account_id, \
             interval_secs, last_poll_at, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&c.device_code_hash)
        .bind(&c.user_code_hash)
        .bind(&c.client_name)
        .bind(&c.status)
        .bind(&c.account_id)
        .bind(c.interval_secs)
        .bind(c.last_poll_at)
        .bind(c.expires_at)
        .bind(c.created_at)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn device_code(&self, device_code_hash: &str) -> RepoResult<Option<DeviceCodeRow>> {
        let row = sqlx::query("SELECT * FROM device_codes WHERE device_code_hash = ?")
            .bind(device_code_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.as_ref().map(device_code_row))
    }

    async fn device_code_by_user_code(
        &self,
        user_code_hash: &str,
    ) -> RepoResult<Option<DeviceCodeRow>> {
        let row = sqlx::query("SELECT * FROM device_codes WHERE user_code_hash = ?")
            .bind(user_code_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.as_ref().map(device_code_row))
    }

    async fn transition_device_code(
        &self,
        device_code_hash: &str,
        from: &str,
        to: &str,
        account_id: Option<&str>,
    ) -> RepoResult<bool> {
        let r = sqlx::query(
            "UPDATE device_codes SET status = ?, account_id = COALESCE(?, account_id) \
             WHERE device_code_hash = ? AND status = ?",
        )
        .bind(to)
        .bind(account_id)
        .bind(device_code_hash)
        .bind(from)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(r.rows_affected() == 1)
    }

    async fn record_device_code_poll(
        &self,
        device_code_hash: &str,
        now: i64,
        interval_secs: i64,
    ) -> RepoResult<()> {
        sqlx::query(
            "UPDATE device_codes SET last_poll_at = ?, interval_secs = ? WHERE device_code_hash = ?",
        )
        .bind(now)
        .bind(interval_secs)
        .bind(device_code_hash)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn audit(&self, e: &AuditEntry) -> RepoResult<()> {
        sqlx::query(
            "INSERT INTO audit_log (at, event, account_id, device_id, ip, detail) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(e.at)
        .bind(&e.event)
        .bind(&e.account_id)
        .bind(&e.device_id)
        .bind(&e.ip)
        .bind(&e.detail)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn audit_for_account(&self, account_id: &str, limit: i64) -> RepoResult<Vec<AuditEntry>> {
        let rows = sqlx::query(
            "SELECT at, event, account_id, device_id, ip, detail FROM audit_log \
             WHERE account_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(account_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .iter()
            .map(|r| AuditEntry {
                at: r.get("at"),
                event: r.get("event"),
                account_id: r.get("account_id"),
                device_id: r.get("device_id"),
                ip: r.get("ip"),
                detail: r.get("detail"),
            })
            .collect())
    }

    async fn purge(&self, now: i64, mailbox_cutoff: i64) -> RepoResult<u64> {
        let statements: [(&'static str, i64); 7] = [
            // Accounts never verified within a week (and never linked to a
            // provider, which marks them verified).
            (
                "DELETE FROM accounts WHERE email_verified = 0 AND created_at <= ? - 604800",
                now,
            ),
            ("DELETE FROM access_tokens WHERE expires_at <= ?", now),
            ("DELETE FROM refresh_tokens WHERE expires_at <= ?", now),
            ("DELETE FROM email_tokens WHERE expires_at <= ?", now),
            ("DELETE FROM ephemeral WHERE expires_at <= ?", now),
            // Keep expired device codes an hour so late polls get expired_token.
            ("DELETE FROM device_codes WHERE expires_at <= ? - 3600", now),
            ("DELETE FROM mailbox WHERE created_at <= ?", mailbox_cutoff),
        ];
        let mut removed = 0;
        for (sql, arg) in statements {
            removed += sqlx::query(sql)
                .bind(arg)
                .execute(&self.pool)
                .await
                .map_err(db)?
                .rows_affected();
        }
        Ok(removed)
    }

    async fn ping(&self) -> RepoResult<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }
}
