use axum::{
    extract::{Path, State},
    Json,
};
use uuid::Uuid;

use crate::{
    errors::MailError,
    middleware::AuthUser,
    models::{CreateAccountDto, EmailAccount, TestConnectionDto, UpdateAccountDto},
    services::{crypto::MailCrypto, imap_service::{self, ImapConfig}},
    state::AppState,
};

// TLS for POP3 raw connection test
use native_tls;
use tokio_native_tls;

pub async fn list_accounts(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, MailError> {
    let accounts = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE user_id = $1 ORDER BY created_at"#,
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "accounts": accounts })))
}

pub async fn create_account(
    State(state): State<AppState>,
    user: AuthUser,
    Json(dto): Json<CreateAccountDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    if dto.name.trim().is_empty() {
        return Err(MailError::Validation("Le nom est requis".into()));
    }
    if dto.email_address.trim().is_empty() || !dto.email_address.contains('@') {
        return Err(MailError::Validation("Adresse email invalide".into()));
    }

    let crypto = MailCrypto::new(&state.settings.mail.encryption_key)
        .map_err(|_| MailError::Crypto)?;

    let (imap_enc, imap_nonce) = crypto.encrypt(&dto.imap_password).map_err(|_| MailError::Crypto)?;
    let (smtp_enc, smtp_nonce) = crypto.encrypt(&dto.smtp_password).map_err(|_| MailError::Crypto)?;

    let is_default    = dto.is_default.unwrap_or(false);
    let imap_port     = dto.imap_port.unwrap_or(993);
    let smtp_port     = dto.smtp_port.unwrap_or(587);
    let imap_sec      = dto.imap_security.as_deref().unwrap_or("ssl").to_string();
    let smtp_sec      = dto.smtp_security.as_deref().unwrap_or("starttls").to_string();
    let incoming_prot = dto.incoming_protocol.as_deref().unwrap_or("imap").to_string();

    let mut tx = state.db.begin().await?;

    if is_default {
        sqlx::query("UPDATE mail.accounts SET is_default = FALSE WHERE user_id = $1")
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }

    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO mail.accounts
           (id, user_id, name, email_address, incoming_protocol,
            imap_host, imap_port, imap_security, imap_username, imap_password, imap_password_nonce,
            smtp_host, smtp_port, smtp_security, smtp_username, smtp_password, smtp_password_nonce,
            is_default)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)"#,
    )
    .bind(id)
    .bind(user.id)
    .bind(&dto.name)
    .bind(&dto.email_address)
    .bind(&incoming_prot)
    .bind(&dto.imap_host)
    .bind(imap_port)
    .bind(&imap_sec)
    .bind(&dto.imap_username)
    .bind(imap_enc.as_slice())
    .bind(imap_nonce.as_slice())
    .bind(&dto.smtp_host)
    .bind(smtp_port)
    .bind(&smtp_sec)
    .bind(&dto.smtp_username)
    .bind(smtp_enc.as_slice())
    .bind(smtp_nonce.as_slice())
    .bind(is_default)
    .execute(&mut *tx)
    .await?;

    // Créer les labels système
    for (name, folder) in &[
        ("Boîte de réception", "INBOX"),
        ("Envoyés",            "Sent"),
        ("Brouillons",         "Drafts"),
        ("Spam",               "Junk"),
        ("Corbeille",          "Trash"),
    ] {
        sqlx::query(
            "INSERT INTO mail.labels (account_id, user_id, name, imap_folder, is_system) VALUES ($1,$2,$3,$4,TRUE)"
        )
        .bind(id)
        .bind(user.id)
        .bind(name)
        .bind(folder)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(Json(serde_json::json!({ "id": id, "message": "Compte créé" })))
}

pub async fn get_account(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<Uuid>,
) -> Result<Json<EmailAccount>, MailError> {
    let account = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE id = $1 AND user_id = $2"#,
    )
    .bind(account_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Compte {account_id}")))?;

    Ok(Json(account))
}

pub async fn test_existing_account(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<Uuid>,
    Json(dto): Json<TestConnectionDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    // Charger les credentials chiffrés depuis la DB
    let row = sqlx::query(
        r#"SELECT incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  imap_password, imap_password_nonce,
                  smtp_host, smtp_port, smtp_security, smtp_username,
                  smtp_password, smtp_password_nonce
           FROM mail.accounts WHERE id = $1 AND user_id = $2"#,
    )
    .bind(account_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Compte {account_id}")))?;

    let crypto = MailCrypto::new(&state.settings.mail.encryption_key)
        .map_err(|_| MailError::Crypto)?;

    use sqlx::Row;
    let imap_enc:   Vec<u8> = row.try_get("imap_password").map_err(anyhow::Error::from)?;
    let imap_nonce: Vec<u8> = row.try_get("imap_password_nonce").map_err(anyhow::Error::from)?;
    let smtp_enc:   Vec<u8> = row.try_get("smtp_password").map_err(anyhow::Error::from)?;
    let smtp_nonce: Vec<u8> = row.try_get("smtp_password_nonce").map_err(anyhow::Error::from)?;

    let imap_pass = crypto.decrypt(&imap_enc, &imap_nonce).map_err(|_| MailError::Crypto)?;
    let smtp_pass = crypto.decrypt(&smtp_enc, &smtp_nonce).map_err(|_| MailError::Crypto)?;

    // Les champs serveur viennent du formulaire (peuvent avoir été modifiés),
    // les mots de passe viennent de la DB si le formulaire les a laissés vides
    let effective = TestConnectionDto {
        incoming_protocol: if dto.incoming_protocol.is_none() {
            row.try_get::<String,_>("incoming_protocol").ok()
        } else {
            dto.incoming_protocol
        },
        imap_host:     if dto.imap_host.is_empty()     { row.try_get("imap_host").map_err(anyhow::Error::from)? } else { dto.imap_host },
        imap_port:     if dto.imap_port.is_none()      { row.try_get::<i32,_>("imap_port").ok() } else { dto.imap_port },
        imap_security: if dto.imap_security.is_none()  { row.try_get::<String,_>("imap_security").ok() } else { dto.imap_security },
        imap_username: if dto.imap_username.is_empty() { row.try_get("imap_username").map_err(anyhow::Error::from)? } else { dto.imap_username },
        imap_password: if dto.imap_password.is_empty() { imap_pass } else { dto.imap_password },
        smtp_host:     if dto.smtp_host.is_empty()     { row.try_get("smtp_host").map_err(anyhow::Error::from)? } else { dto.smtp_host },
        smtp_port:     if dto.smtp_port.is_none()      { row.try_get::<i32,_>("smtp_port").ok() } else { dto.smtp_port },
        smtp_security: if dto.smtp_security.is_none()  { row.try_get::<String,_>("smtp_security").ok() } else { dto.smtp_security },
        smtp_username: if dto.smtp_username.is_empty() { row.try_get("smtp_username").map_err(anyhow::Error::from)? } else { dto.smtp_username },
        smtp_password: if dto.smtp_password.is_empty() { smtp_pass } else { dto.smtp_password },
    };

    run_connection_test(effective).await
}

pub async fn update_account(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<Uuid>,
    Json(dto): Json<UpdateAccountDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    let exists: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM mail.accounts WHERE id = $1 AND user_id = $2)"
    )
    .bind(account_id)
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;

    if !exists {
        return Err(MailError::NotFound(format!("Compte {account_id}")));
    }

    let crypto = MailCrypto::new(&state.settings.mail.encryption_key)
        .map_err(|_| MailError::Crypto)?;

    // Chiffrer les mots de passe uniquement s'ils sont fournis et non vides
    let imap_pass_enc = match &dto.imap_password {
        Some(p) if !p.is_empty() => Some(crypto.encrypt(p).map_err(|_| MailError::Crypto)?),
        _ => None,
    };
    let smtp_pass_enc = match &dto.smtp_password {
        Some(p) if !p.is_empty() => Some(crypto.encrypt(p).map_err(|_| MailError::Crypto)?),
        _ => None,
    };

    // Détecter un changement d'IDENTIFIANTS DE CONNEXION IMAP (serveur, port, login ou
    // mot de passe). Si oui, les messages déjà synchronisés proviennent potentiellement
    // d'une AUTRE boîte → on les purge et on relance un sync complet (cf. plus bas).
    let (cur_host, cur_port, cur_user, cur_pass_enc, cur_pass_nonce):
        (String, i32, String, Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT imap_host, imap_port, imap_username, imap_password, imap_password_nonce \
         FROM mail.accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;

    let mut creds_changed = false;
    if let Some(h) = dto.imap_host.as_deref()     { creds_changed |= !h.is_empty() && h != cur_host; }
    if let Some(p) = dto.imap_port                { creds_changed |= p != cur_port; }
    if let Some(u) = dto.imap_username.as_deref()  { creds_changed |= !u.is_empty() && u != cur_user; }
    if let Some(new_pass) = dto.imap_password.as_deref() {
        if !new_pass.is_empty() {
            let cur_plain = crypto.decrypt(&cur_pass_enc, &cur_pass_nonce).unwrap_or_default();
            creds_changed |= new_pass != cur_plain;
        }
    }

    let mut tx = state.db.begin().await?;

    macro_rules! update_field {
        ($col:literal, $val:expr) => {
            if let Some(v) = $val {
                sqlx::query(concat!("UPDATE mail.accounts SET ", $col, " = $1 WHERE id = $2"))
                    .bind(v).bind(account_id).execute(&mut *tx).await?;
            }
        };
    }

    update_field!("name",              dto.name.as_deref());
    update_field!("email_address",     dto.email_address.as_deref());
    update_field!("incoming_protocol", dto.incoming_protocol.as_deref());
    update_field!("imap_host",         dto.imap_host.as_deref());
    update_field!("imap_port",     dto.imap_port);
    update_field!("imap_security", dto.imap_security.as_deref());
    update_field!("imap_username", dto.imap_username.as_deref());
    update_field!("smtp_host",     dto.smtp_host.as_deref());
    update_field!("smtp_port",     dto.smtp_port);
    update_field!("smtp_security", dto.smtp_security.as_deref());
    update_field!("smtp_username", dto.smtp_username.as_deref());

    if let Some((enc, nonce)) = &imap_pass_enc {
        sqlx::query("UPDATE mail.accounts SET imap_password = $1, imap_password_nonce = $2 WHERE id = $3")
            .bind(enc.as_slice()).bind(nonce.as_slice()).bind(account_id).execute(&mut *tx).await?;
    }
    if let Some((enc, nonce)) = &smtp_pass_enc {
        sqlx::query("UPDATE mail.accounts SET smtp_password = $1, smtp_password_nonce = $2 WHERE id = $3")
            .bind(enc.as_slice()).bind(nonce.as_slice()).bind(account_id).execute(&mut *tx).await?;
    }

    if let Some(default) = dto.is_default {
        sqlx::query("UPDATE mail.accounts SET is_default = FALSE WHERE user_id = $1")
            .bind(user.id).execute(&mut *tx).await?;
        if default {
            sqlx::query("UPDATE mail.accounts SET is_default = TRUE WHERE id = $1")
                .bind(account_id).execute(&mut *tx).await?;
        }
    }

    // Identifiants de connexion modifiés → purger les contenus SYNCHRONISÉS (threads +
    // messages ; les thread_labels suivent en cascade). Les brouillons de l'utilisateur
    // (drafts) sont conservés (leur message_id passe à NULL). last_sync_at remis à NULL
    // pour que le worker refasse un sync complet depuis la nouvelle boîte.
    if creds_changed {
        sqlx::query("DELETE FROM mail.messages WHERE account_id = $1")
            .bind(account_id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM mail.threads WHERE account_id = $1")
            .bind(account_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE mail.accounts SET last_sync_at = NULL, last_error = NULL WHERE id = $1")
            .bind(account_id).execute(&mut *tx).await?;
    }

    tx.commit().await?;
    Ok(Json(serde_json::json!({
        "message": "Compte mis à jour",
        "resynced": creds_changed,
    })))
}

pub async fn delete_account(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let result = sqlx::query("DELETE FROM mail.accounts WHERE id = $1 AND user_id = $2")
        .bind(account_id)
        .bind(user.id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(MailError::NotFound(format!("Compte {account_id}")));
    }
    Ok(Json(serde_json::json!({ "message": "Compte supprimé" })))
}

// ── POP3 connection test (raw TCP + optional TLS) ───────────────────────────

async fn pop3_read_line<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> anyhow::Result<String> {
    use tokio::io::AsyncReadExt;
    let mut line = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await?;
        line.push(byte[0]);
        let len = line.len();
        if len >= 2 && line[len - 2] == b'\r' && line[len - 1] == b'\n' { break; }
        if len > 2048 { break; }
    }
    Ok(String::from_utf8_lossy(&line).to_string())
}

async fn pop3_auth_stream<S>(stream: &mut S, user: &str, pass: &str, t: std::time::Duration) -> (bool, Option<String>)
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin
{
    use tokio::io::AsyncWriteExt;

    // Read greeting
    match tokio::time::timeout(t, pop3_read_line(stream)).await {
        Ok(Ok(l)) if l.starts_with("+OK") => {}
        Ok(Ok(l)) => return (false, Some(format!("Réponse inattendue: {}", l.trim()))),
        Ok(Err(e)) => return (false, Some(e.to_string())),
        Err(_) => return (false, Some("Délai d'attente dépassé".into())),
    }
    // USER
    let _ = stream.write_all(format!("USER {user}\r\n").as_bytes()).await;
    match tokio::time::timeout(t, pop3_read_line(stream)).await {
        Ok(Ok(l)) if l.starts_with("+OK") => {}
        Ok(Ok(l)) => { let _ = stream.write_all(b"QUIT\r\n").await; return (false, Some(l.trim().to_string())); }
        _ => return (false, Some("Erreur lecture USER".into())),
    }
    // PASS
    let _ = stream.write_all(format!("PASS {pass}\r\n").as_bytes()).await;
    let (ok, err) = match tokio::time::timeout(t, pop3_read_line(stream)).await {
        Ok(Ok(l)) if l.starts_with("+OK") => (true, None),
        Ok(Ok(l)) => (false, Some(l.trim().to_string())),
        _ => (false, Some("Délai d'attente dépassé".into())),
    };
    let _ = stream.write_all(b"QUIT\r\n").await;
    (ok, err)
}

async fn test_pop3(dto: &TestConnectionDto) -> (bool, Option<String>) {
    let host     = &dto.imap_host;
    let port     = dto.imap_port.unwrap_or(995) as u16;
    let security = dto.imap_security.as_deref().unwrap_or("ssl");
    let user     = &dto.imap_username;
    let pass     = &dto.imap_password;
    let t        = std::time::Duration::from_secs(10);
    let addr     = format!("{host}:{port}");

    if security == "ssl" {
        let tcp = match tokio::time::timeout(t, tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return (false, Some(format!("Connexion refusée: {e}"))),
            Err(_) => return (false, Some("Délai de connexion dépassé".into())),
        };
        let connector = tokio_native_tls::TlsConnector::from(
            match native_tls::TlsConnector::new() {
                Ok(c) => c,
                Err(e) => return (false, Some(format!("TLS init: {e}"))),
            }
        );
        let mut tls = match connector.connect(host, tcp).await {
            Ok(s) => s,
            Err(e) => return (false, Some(format!("TLS: {e}"))),
        };
        pop3_auth_stream(&mut tls, user, pass, t).await
    } else {
        let mut tcp = match tokio::time::timeout(t, tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return (false, Some(format!("Connexion refusée: {e}"))),
            Err(_) => return (false, Some("Délai de connexion dépassé".into())),
        };
        pop3_auth_stream(&mut tcp, user, pass, t).await
    }
}

// ── TCP-only connectivity probe (no TLS, no auth) ────────────────────────────

async fn test_tcp_connect(host: &str, port: u16) -> (bool, Option<String>) {
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::TcpStream::connect(format!("{host}:{port}")),
    ).await {
        Ok(Ok(_)) => (true, None),
        Ok(Err(e)) => (false, Some(format!("Hôte inaccessible: {e}"))),
        Err(_) => (false, Some("Délai de connexion dépassé".into())),
    }
}

// ── Main test dispatcher ─────────────────────────────────────────────────────

async fn run_connection_test(dto: TestConnectionDto) -> Result<Json<serde_json::Value>, MailError> {
    use lettre::{transport::smtp::authentication::Credentials, AsyncSmtpTransport, Tokio1Executor};
    let timeout = std::time::Duration::from_secs(10);

    let protocol      = dto.incoming_protocol.as_deref().unwrap_or("imap");
    let incoming_port = dto.imap_port.unwrap_or(if protocol == "pop3" { 995 } else { 993 }) as u16;
    let smtp_port     = dto.smtp_port.unwrap_or(587) as u16;

    // ── Connexion TCP (indépendant des credentials) ──────────────────────────
    let (inc_conn_ok,  inc_conn_err)  = test_tcp_connect(&dto.imap_host, incoming_port).await;
    let (smtp_conn_ok, smtp_conn_err) = test_tcp_connect(&dto.smtp_host, smtp_port).await;

    // ── Auth entrante ────────────────────────────────────────────────────────
    let (inc_auth_ok, inc_auth_err) = if !inc_conn_ok {
        (false, Some("Serveur inaccessible".into()))
    } else if protocol == "pop3" {
        test_pop3(&dto).await
    } else {
        let imap_cfg = ImapConfig {
            host:     dto.imap_host.clone(),
            port:     incoming_port,
            security: dto.imap_security.clone().unwrap_or_else(|| "ssl".into()),
            username: dto.imap_username.clone(),
            password: dto.imap_password.clone(),
        };
        match tokio::time::timeout(timeout, imap_service::connect(&imap_cfg)).await {
            Ok(Ok(session)) => { imap_service::logout(session).await; (true, None) }
            Ok(Err(e))      => (false, Some(e.to_string())),
            Err(_)          => (false, Some("Délai d'attente dépassé (10s)".into())),
        }
    };

    // ── Auth SMTP ────────────────────────────────────────────────────────────
    let (smtp_auth_ok, smtp_auth_err) = if !smtp_conn_ok {
        (false, Some("Serveur inaccessible".into()))
    } else {
        let creds = Credentials::new(dto.smtp_username.clone(), dto.smtp_password.clone());
        let transport_result: anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> =
            match dto.smtp_security.as_deref().unwrap_or("starttls") {
                "ssl" => AsyncSmtpTransport::<Tokio1Executor>::relay(&dto.smtp_host)
                    .map_err(anyhow::Error::from)
                    .map(|b| b.port(smtp_port).credentials(creds).build()),
                _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&dto.smtp_host)
                    .map_err(anyhow::Error::from)
                    .map(|b| b.port(smtp_port).credentials(creds).build()),
            };
        match transport_result {
            Err(e) => (false, Some(e.to_string())),
            Ok(transport) => match tokio::time::timeout(timeout, transport.test_connection()).await {
                Ok(Ok(true))  => (true,  None),
                Ok(Ok(false)) => (false, Some("Le serveur SMTP a refusé la connexion".into())),
                Ok(Err(e))    => (false, Some(e.to_string())),
                Err(_)        => (false, Some("Délai d'attente dépassé (10s)".into())),
            },
        }
    };

    Ok(Json(serde_json::json!({
        "incoming": {
            "protocol":   protocol,
            "connection": { "ok": inc_conn_ok,  "error": inc_conn_err  },
            "auth":       { "ok": inc_auth_ok,  "error": inc_auth_err  },
        },
        "smtp": {
            "connection": { "ok": smtp_conn_ok, "error": smtp_conn_err },
            "auth":       { "ok": smtp_auth_ok, "error": smtp_auth_err },
        },
    })))
}

pub async fn test_connection(
    _state: State<AppState>,
    _user: AuthUser,
    Json(dto): Json<TestConnectionDto>,
) -> Result<Json<serde_json::Value>, MailError> {
    run_connection_test(dto).await
}

pub async fn trigger_sync(
    State(state): State<AppState>,
    user: AuthUser,
    Path(account_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, MailError> {
    let account = sqlx::query_as::<_, EmailAccount>(
        r#"SELECT id, user_id, name, email_address,
                  incoming_protocol,
                  imap_host, imap_port, imap_security, imap_username,
                  smtp_host, smtp_port, smtp_security, smtp_username,
                  is_default, is_active, last_sync_at, last_error,
                  created_at, updated_at
           FROM mail.accounts WHERE id = $1 AND user_id = $2"#,
    )
    .bind(account_id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| MailError::NotFound(format!("Compte {account_id}")))?;

    let db2 = state.db.clone();
    let key  = state.settings.mail.encryption_key.clone();
    let mail_cfg = state.settings.mail.clone();
    tokio::spawn(async move {
        let crypto = match MailCrypto::new(&key) {
            Ok(c)  => c,
            Err(e) => { tracing::error!(error = %e, "Sync: clé invalide"); return; }
        };
        if let Err(e) = crate::services::sync_service::sync_account(&db2, &account, &crypto, &mail_cfg).await {
            tracing::error!(account_id = %account.id, error = %e, "Sync manuel: erreur");
        }
    });

    Ok(Json(serde_json::json!({ "message": "Synchronisation démarrée" })))
}
