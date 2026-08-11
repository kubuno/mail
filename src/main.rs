use anyhow::{Context, Result};
use clap::Parser;
use kubuno_mail::{
    config::Settings,
    router,
    state::AppState,
    workers::{sync_worker, scheduler_worker},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;

// ── Lecture de module.toml ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Manifest {
    module:        ManifestModule,
    #[serde(default)]
    sidebar_items: Vec<SidebarItemRaw>,
    events:        Option<ManifestEvents>,
    /// Declarative instance settings, edited in the admin console and read back
    /// by the module through /internal/modules/settings.
    #[serde(default)]
    settings:      Vec<SettingDefRaw>,
    /// Pages the admin panel of this module is split into (`[[setting_groups]]`).
    /// Each becomes an entry of the admin menu with its own address; the
    /// `category` of a setting becomes a tab inside its group.
    #[serde(default)]
    setting_groups: Vec<SettingGroupRaw>,
}

/// One `[[setting_groups]]` entry of module.toml, forwarded verbatim.
///
/// `id` is a STABLE, UNTRANSLATED slug: it travels in the URL of the admin page,
/// which may not change shape with the interface language. The core refuses a
/// registration whose group id is not a slug, or whose settings point at a group
/// that was never declared.
#[derive(Deserialize, Serialize)]
struct SettingGroupRaw {
    id:          String,
    label:       String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    icon:        Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position:    Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// One `[[settings]]` entry of module.toml, forwarded verbatim at registration
/// (`type` renamed to match the core's `SettingDef`).
#[derive(Deserialize, Serialize)]
struct SettingDefRaw {
    key:         String,
    scope:       String,
    #[serde(rename = "type")]
    value_type:  String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    values:      Option<Value>,
    default:     Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label:       Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category:    Option<String>,
    /// Id of a `[[setting_groups]]` entry: which page of the panel this belongs
    /// to. Absent = ungrouped, as before groups existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group:       Option<String>,
    #[serde(default)]
    public:      bool,
    // ── Presentation metadata ───────────────────────────────────────────────
    // The panel is schema-driven: these travel to the core untouched and are
    // what let it render fifty settings without a line of module-specific
    // front-end code. Anything the core does not understand it ignores, so an
    // older core still shows the setting, just plainly.
    /// Fold behind the section's "advanced" disclosure.
    #[serde(default)]
    advanced:    bool,
    /// "info" | "warning" | "danger" — how loudly to warn before changing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    risk:        Option<String>,
    /// Bounds for `type = "int"`, enforced by the core as well as the panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min:         Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max:         Option<i64>,
    /// Suffix shown beside the field ("Mo", "s", "min").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unit:        Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placeholder: Option<String>,
    /// The string value is a list, one entry per line — render a textarea.
    #[serde(default)]
    multiline:   bool,
    /// Key of a boolean setting of the same module; hidden while it is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    depends_on:  Option<String>,
}

#[derive(Deserialize)]
struct ManifestModule {
    #[allow(dead_code)]
    id:            String,
    display_name:  String,
    description:   Option<String>,
    settings_path: Option<String>,
}

#[derive(Deserialize)]
struct SidebarItemRaw {
    id:       String,
    label:    String,
    icon:     String,
    path:     String,
    position: i32,
    /// `false` for internal views/folders (Sent, Drafts, Trash…) that are not
    /// launchable apps. Defaults to `true` for backward compatibility.
    #[serde(default = "default_launchable")]
    launchable: bool,
}

fn default_launchable() -> bool {
    true
}

#[derive(Deserialize)]
struct ManifestEvents {
    #[serde(default)]
    subscribed: Vec<String>,
}

fn load_manifest() -> Option<Manifest> {
    let path = if let Ok(dir) = std::env::var("KUBUNO_MODULE_DIR") {
        std::path::PathBuf::from(dir).join("module.toml")
    } else {
        std::env::current_exe().ok()?.parent()?.join("module.toml")
    };

    let content = std::fs::read_to_string(&path)
        .map_err(|e| tracing::warn!(path = %path.display(), error = %e, "module.toml introuvable"))
        .ok()?;

    toml::from_str::<Manifest>(&content)
        .map_err(|e| tracing::error!(path = %path.display(), error = %e, "module.toml invalide"))
        .ok()
}

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "kubuno-mail", version, about = "Module mail Kubuno")]
struct Cli {
    #[arg(short, long, env = "KM_CONFIG_FILE")]
    config: Option<String>,
}

// ── Point d'entrée ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let _cli = Cli::parse();

    let settings = Settings::load().context("Chargement de la configuration")?;

    let log_level = settings.logging.level.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&log_level)),
        );

    match settings.logging.format {
        kubuno_mail::config::LogFormat::Json   => subscriber.json().init(),
        kubuno_mail::config::LogFormat::Pretty => subscriber.init(),
    }

    tracing::info!("Kubuno Mail v{} démarrage…", env!("CARGO_PKG_VERSION"));

    // Sécurité : interdire toute exécution de processus sur l’hôte (voir kubuno-seccomp).
    kubuno_seccomp::lock_down_process_execution("mail");

    // Pool PostgreSQL
    let opts = settings.database.connect_options()?;
    let pool = PgPoolOptions::new()
        .max_connections(settings.database.max_connections)
        .min_connections(settings.database.min_connections)
        .acquire_timeout(settings.database.connect_timeout)
        .connect_with(opts)
        .await
        .context("Connexion PostgreSQL")?;

    // Migrations
    if settings.database.run_migrations {
        sqlx::query("CREATE SCHEMA IF NOT EXISTS mail")
            .execute(&pool)
            .await
            .context("Création du schéma mail")?;

        let migration_opts = settings.database.connect_options()?
            .options([("search_path", "mail,public")]);
        let migration_pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(settings.database.connect_timeout)
            .connect_with(migration_opts)
            .await
            .context("Pool de migration")?;

        sqlx::migrate!("./migrations")
            .run(&migration_pool)
            .await
            .context("Migrations")?;
    }

    let state = AppState {
        db:       pool,
        settings: Arc::new(settings.clone()),
    };

    // Backfill: give every active mailbox created before migration 000025 the
    // local account that fronts it, so it appears in the account list and the
    // "From" selector. Idempotent, so it runs on every boot regardless of
    // `run_migrations`. A global failure is logged and does not block startup.
    if let Err(e) = kubuno_mail::handlers::addresses::mailboxes::ensure_local_accounts(&state).await {
        tracing::error!(error = %e, "backfill des comptes locaux (démarrage poursuivi)");
    }

    // Enregistrement auprès du core (avec retry infini)
    let http = Client::new();
    register_with_core(&http, &settings).await;

    // Heartbeat toutes les 30s
    {
        let http2     = http.clone();
        let settings2 = settings.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let url    = format!("{}/internal/modules/mail/heartbeat", settings2.core.url);
                let secret = &settings2.core.internal_secret;
                match http2.post(&url).header("X-Internal-Secret", secret.as_str()).send().await {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {
                        tracing::info!("Heartbeat 404 — ré-enregistrement…");
                        register_with_core(&http2, &settings2).await;
                    }
                    Ok(r) if r.status() == reqwest::StatusCode::FORBIDDEN => {
                        tracing::info!("Heartbeat 403 — module désactivé, attente…");
                    }
                    Ok(r)  => tracing::warn!(status = %r.status(), "Heartbeat réponse inattendue"),
                    Err(e) => tracing::warn!(error = %e, "Heartbeat erreur réseau"),
                }
            }
        });
    }

    // Worker de synchronisation IMAP
    {
        let state2 = Arc::new(state.clone());
        tokio::spawn(async move {
            sync_worker::run(state2).await;
        });
    }

    // Worker d'envoi programmé (brouillons avec scheduled_at échu)
    {
        let state3 = Arc::new(state.clone());
        tokio::spawn(async move {
            scheduler_worker::run(state3).await;
        });
    }

    // SMTP/IMAP/POP3 services. Nothing listens until an administrator switches
    // one on in the console: the supervisor reads that configuration and
    // reconciles its listeners with it.
    {
        let db     = state.db.clone();
        let cfg    = settings.clone();
        let client = http.clone();
        tokio::spawn(async move {
            kubuno_mail::server::run(db, cfg, client).await;
        });
    }

    // Outbound queue worker: delivers queued mail to remote servers with
    // retries and DSNs. Idle until an administrator enables outbound sending.
    {
        let db     = state.db.clone();
        let cfg    = settings.clone();
        let client = http.clone();
        tokio::spawn(async move {
            kubuno_mail::server::worker::run(db, cfg, client).await;
        });
    }

    // Serveur HTTP
    let addr = format!("{}:{}", settings.server.host, settings.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("Bind sur {addr}"))?;

    tracing::info!("Kubuno Mail démarré sur http://{addr}");

    let app = router::build(state);
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await
        .context("Erreur du serveur HTTP")?;

    Ok(())
}

fn backoff(attempt: u32) -> u64 {
    if attempt <= 10 { (attempt * 2) as u64 } else { 30 }
}

async fn register_with_core(http: &Client, settings: &Settings) {
    let base_url = format!("http://{}:{}", settings.server.host, settings.server.port);
    let core_url = &settings.core.url;
    let secret   = &settings.core.internal_secret;

    let manifest = load_manifest();
    let display_name  = manifest.as_ref().map(|m| m.module.display_name.as_str()).unwrap_or("Mail").to_string();
    let description   = manifest.as_ref().and_then(|m| m.module.description.clone());
    let settings_path = manifest.as_ref().and_then(|m| m.module.settings_path.clone());
    let sidebar_items: Vec<Value> = manifest.as_ref()
        .map(|m| m.sidebar_items.iter().map(|s| json!({
            "id":       s.id,
            "label":    s.label,
            "icon":     s.icon,
            "path":     s.path,
            "position": s.position,
            "launchable": s.launchable,
        })).collect())
        .unwrap_or_else(|| vec![
            json!({ "id": "mail-inbox", "label": "Boîte de réception", "icon": "Inbox", "path": "/mail", "position": 30, "launchable": true }),
        ]);
    let subscribed_events: Vec<String> = manifest.as_ref()
        .and_then(|m| m.events.as_ref())
        .map(|e| e.subscribed.clone())
        .unwrap_or_else(|| vec!["UserDeleted".into()]);

    let settings_schema: Value = manifest.as_ref()
        .map(|m| serde_json::to_value(&m.settings).unwrap_or_else(|_| json!([])))
        .unwrap_or_else(|| json!([]));

    // Pages of the admin panel. An older core ignores the field, and the module
    // then keeps the single-page panel it had — nothing here is required.
    let setting_groups: Value = manifest.as_ref()
        .map(|m| serde_json::to_value(&m.setting_groups).unwrap_or_else(|_| json!([])))
        .unwrap_or_else(|| json!([]));

    let payload = json!({
        "module_id":         "mail",
        "display_name":      display_name,
        "description":       description,
        "settings_path":     settings_path,
        "settings_schema":   settings_schema,
        "setting_groups":    setting_groups,
        "base_url":          base_url,
        "version":           env!("CARGO_PKG_VERSION"),
        "routes":            [{ "method": "*", "path": "/*" }],
        "sidebar_items":     sidebar_items,
        "subscribed_events": subscribed_events,
    });

    for attempt in 1u32.. {
        let url = format!("{core_url}/internal/modules/register");
        match http.post(&url)
            .header("X-Internal-Secret", secret.as_str())
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("Module mail enregistré auprès du core");
                return;
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::FORBIDDEN => {
                tracing::info!(attempt, "Module désactivé par l'admin, nouvel essai dans 30s…");
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            Ok(resp) => {
                let wait = backoff(attempt);
                tracing::warn!(attempt, status = %resp.status(), "Enregistrement échoué, retry dans {wait}s…");
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
            Err(e) => {
                let wait = backoff(attempt);
                tracing::warn!(attempt, error = %e, "Core inaccessible, retry dans {wait}s…");
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real `module.toml`, parsed by the very structs used at registration.
    ///
    /// This is what the core receives, so a typo here would be found by an
    /// administrator missing a page rather than by a build: the manifest is
    /// data, and nothing else type-checks it.
    fn manifest() -> Manifest {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("module.toml");
        let content = std::fs::read_to_string(&path).expect("module.toml lisible");
        toml::from_str::<Manifest>(&content).expect("module.toml valide")
    }

    #[test]
    fn manifest_declares_its_admin_groups() {
        let m = manifest();
        assert!(!m.setting_groups.is_empty(), "aucun groupe déclaré");
        for g in &m.setting_groups {
            assert!(!g.label.trim().is_empty(), "groupe '{}' sans libellé", g.id);
            assert!(
                g.id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "l'id de groupe '{}' n'est pas un slug d'URL",
                g.id
            );
        }
    }

    /// The same rule the core enforces at registration, checked here so a bad
    /// manifest fails the module's own build rather than its next start-up.
    #[test]
    fn every_setting_group_reference_resolves() {
        let m = manifest();
        let ids: Vec<&str> = m.setting_groups.iter().map(|g| g.id.as_str()).collect();
        for s in &m.settings {
            if let Some(g) = &s.group {
                assert!(
                    ids.contains(&g.as_str()),
                    "le réglage '{}' pointe le groupe inconnu '{g}'",
                    s.key
                );
            }
        }
    }

    /// The registration payload keeps the manifest's own field names — the core
    /// deserialises `SettingGroup`/`SettingDef` straight from them.
    #[test]
    fn groups_serialise_under_the_names_the_core_expects() {
        let m = manifest();
        let json = serde_json::to_value(&m.setting_groups).expect("sérialisation");
        let first = &json[0];
        assert!(first["id"].is_string() && first["label"].is_string());
        let settings = serde_json::to_value(&m.settings).expect("sérialisation");
        assert!(settings
            .as_array()
            .expect("tableau")
            .iter()
            .any(|s| s["group"].is_string()));
    }
}
