use anyhow::Context;
use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub server:   ServerSettings,
    pub core:     CoreSettings,
    pub database: DatabaseSettings,
    pub mail:     MailSettings,
    pub logging:  LoggingSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoreSettings {
    pub url:             String,
    pub internal_secret: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSettings {
    pub url:             Option<String>,
    pub host:            Option<String>,
    pub port:            Option<u16>,
    pub user:            Option<String>,
    pub password:        Option<String>,
    pub database:        Option<String>,
    pub max_connections: u32,
    pub min_connections: u32,
    #[serde(with = "duration_secs")]
    pub connect_timeout: Duration,
    pub run_migrations:  bool,
}

impl DatabaseSettings {
    pub fn connect_options(&self) -> anyhow::Result<sqlx::postgres::PgConnectOptions> {
        use std::str::FromStr;
        if self.host.is_some() || self.user.is_some() {
            let user     = self.user.as_deref().context("database.user requis")?;
            let password = self.password.as_deref().context("database.password requis")?;
            let database = self.database.as_deref().context("database.database requis")?;
            return Ok(sqlx::postgres::PgConnectOptions::new()
                .host(self.host.as_deref().unwrap_or("localhost"))
                .port(self.port.unwrap_or(5432))
                .username(user)
                .password(password)
                .database(database));
        }
        if let Some(url) = &self.url {
            return sqlx::postgres::PgConnectOptions::from_str(url)
                .context("database.url invalide");
        }
        Err(anyhow::anyhow!("database : fournissez les champs host/user/password/database"))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MailSettings {
    pub encryption_key:       String,
    pub sync_interval_secs:   u64,
    /// Messages fetched per batch. The whole mailbox is downloaded batch by
    /// batch across runs, so this caps memory per batch, not the total.
    pub max_fetch_per_sync:   u32,
    /// Time budget for one account in one run. Reaching it stops the run
    /// cleanly; cursors are persisted per batch, so the next run resumes.
    pub sync_deadline_secs:   u64,
    /// Directory where incoming attachments are written (served by download_attachment).
    pub attachments_dir:      String,
    /// Public base URL of the Kubuno instance (e.g. "https://dev.kubuno.com"),
    /// used to build the OAuth redirect URIs. When absent, it is derived from
    /// the X-Forwarded-Proto / X-Forwarded-Host / Host headers of the request.
    pub public_base_url:         Option<String>,
    /// OAuth2 client for Gmail accounts (Google Cloud Console, "Web application").
    pub google_client_id:        Option<String>,
    pub google_client_secret:    Option<String>,
    /// OAuth2 client for Outlook.com / Microsoft 365 accounts (Azure app registration).
    pub microsoft_client_id:     Option<String>,
    pub microsoft_client_secret: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingSettings {
    pub level:  String,
    pub format: LogFormat,
}

impl Settings {
    pub fn load() -> Result<Self, ConfigError> {
        let mut builder = Config::builder()
            .set_default("server.host", "127.0.0.1")?
            .set_default("server.port", 3111i64)?
            .set_default("core.url", "http://127.0.0.1:8080")?
            .set_default("core.internal_secret", "")?
            .set_default("database.max_connections", 10i64)?
            .set_default("database.min_connections", 1i64)?
            .set_default("database.connect_timeout", 10i64)?
            .set_default("database.run_migrations", true)?
            .set_default("mail.encryption_key", "")?
            .set_default("mail.sync_interval_secs", 300i64)?
            .set_default("mail.max_fetch_per_sync", 200i64)?
            .set_default("mail.sync_deadline_secs", 240i64)?
            .set_default("mail.attachments_dir", "/var/lib/kubuno/mail/attachments")?
            .set_default("logging.level", "info")?
            .set_default("logging.format", "pretty")?
            .add_source(File::with_name("config").required(false))
            .add_source(File::with_name("/etc/kubuno/modules/mail/config").required(false))
            .add_source(
                Environment::with_prefix("KM")
                    .separator("__")
                    .try_parsing(true),
            );

        // Variables injectées par le superviseur core — priorité maximale
        if let Ok(v) = std::env::var("KUBUNO_CORE_URL")        { builder = builder.set_override("core.url",             v)?; }
        if let Ok(v) = std::env::var("KUBUNO_INTERNAL_SECRET") { builder = builder.set_override("core.internal_secret", v)?; }
        if let Ok(v) = std::env::var("KUBUNO_DB_HOST")         { builder = builder.set_override("database.host",     v)?; }
        if let Ok(v) = std::env::var("KUBUNO_DB_PORT")         { builder = builder.set_override("database.port",     v.parse::<i64>().unwrap_or(5432))?; }
        if let Ok(v) = std::env::var("KUBUNO_DB_USER")         { builder = builder.set_override("database.user",     v)?; }
        if let Ok(v) = std::env::var("KUBUNO_DB_PASSWORD")     { builder = builder.set_override("database.password", v)?; }
        if let Ok(v) = std::env::var("KUBUNO_DB_NAME")         { builder = builder.set_override("database.database", v)?; }

        builder.build()?.try_deserialize()
    }
}

mod duration_secs {
    use serde::{Deserialize, Deserializer};
    use std::time::Duration;
    pub fn deserialize<'de, D>(d: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}
