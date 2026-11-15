//! Outgoing email: a [`Mailer`] trait with an SMTP implementation (lettre),
//! a log mailer for development and a recording mailer for tests.

use crate::config::{MailerConfig, SmtpConfig, SmtpTls};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// A plain-text message.
#[derive(Clone, Debug)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
}

#[async_trait]
pub trait Mailer: Send + Sync + 'static {
    async fn send(&self, email: Email) -> Result<(), String>;
}

/// Builds the mailer the configuration names.
pub fn from_config(cfg: &MailerConfig) -> Result<Arc<dyn Mailer>, String> {
    Ok(match cfg {
        MailerConfig::Smtp(smtp) => Arc::new(SmtpMailer::new(smtp)?),
        MailerConfig::Log => Arc::new(LogMailer),
        MailerConfig::Disabled => Arc::new(DisabledMailer),
    })
}

/// Sends through an SMTP server.
pub struct SmtpMailer {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    from: lettre::message::Mailbox,
}

impl SmtpMailer {
    pub fn new(cfg: &SmtpConfig) -> Result<Self, String> {
        use lettre::AsyncSmtpTransport;
        use lettre::transport::smtp::authentication::Credentials;
        let mut builder = match cfg.tls {
            SmtpTls::Tls => AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(&cfg.host)
                .map_err(|e| e.to_string())?,
            SmtpTls::StartTls => {
                AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(&cfg.host)
                    .map_err(|e| e.to_string())?
            }
            SmtpTls::None => {
                AsyncSmtpTransport::<lettre::Tokio1Executor>::builder_dangerous(&cfg.host)
            }
        }
        .port(cfg.port);
        if let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) {
            builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
        }
        let from = cfg
            .from
            .parse()
            .map_err(|e| format!("RELAY_MAIL_FROM: {e}"))?;
        Ok(Self {
            transport: builder.build(),
            from,
        })
    }
}

#[async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, email: Email) -> Result<(), String> {
        use lettre::AsyncTransport;
        let message = lettre::Message::builder()
            .from(self.from.clone())
            .to(email.to.parse().map_err(|e| format!("recipient: {e}"))?)
            .subject(email.subject)
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(email.text)
            .map_err(|e| e.to_string())?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// Writes messages to the log. For local development only: the log then
/// contains verification and reset links.
pub struct LogMailer;

#[async_trait]
impl Mailer for LogMailer {
    async fn send(&self, email: Email) -> Result<(), String> {
        tracing::warn!(
            to = %email.to,
            subject = %email.subject,
            body = %email.text,
            "development mailer: email not sent"
        );
        Ok(())
    }
}

/// Drops every message.
pub struct DisabledMailer;

#[async_trait]
impl Mailer for DisabledMailer {
    async fn send(&self, _email: Email) -> Result<(), String> {
        Ok(())
    }
}

/// Records messages in memory (tests).
#[derive(Default)]
pub struct MemoryMailer {
    sent: Mutex<Vec<Email>>,
}

impl MemoryMailer {
    pub fn messages(&self) -> Vec<Email> {
        self.sent.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn messages_to(&self, to: &str) -> Vec<Email> {
        self.messages().into_iter().filter(|m| m.to == to).collect()
    }
}

#[async_trait]
impl Mailer for MemoryMailer {
    async fn send(&self, email: Email) -> Result<(), String> {
        if let Ok(mut v) = self.sent.lock() {
            v.push(email);
        }
        Ok(())
    }
}
