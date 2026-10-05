//! Portability proof for the `mail` module across the kubuno-db engines.
//!
//! The SAME body (`exercise`) runs, through the real router and the real
//! server-side store/queue/delivery code, against:
//!
//! * SQLite — always, on a temp directory;
//! * PostgreSQL — when `KUBUNO_PG_TEST_URL` points at a throwaway database;
//! * Oracle MySQL — when `KUBUNO_MYSQL_TEST_URL` does;
//! * MariaDB — when `KUBUNO_MARIADB_TEST_URL` does.
//!
//! Every run uses the schema prefix `kbmig_`, so the module's namespace is
//! `kbmig_mail` (a PostgreSQL schema / a MySQL database) and is dropped and
//! re-created first: point these variables at scratch servers only.
//!
//! ```sh
//! KUBUNO_PG_TEST_URL=postgres://u:p@127.0.0.1:5432/scratch \
//! KUBUNO_MYSQL_TEST_URL=mysql://u:p@127.0.0.1:3306/scratch \
//! KUBUNO_MARIADB_TEST_URL=mysql://u:p@127.0.0.1:3307/scratch \
//!   cargo test --test db_portability -- --test-threads=1
//! ```
//!
//! A fake core answers the two internal endpoints the module reads (the mail
//! server settings and the instance domains), so local delivery, local sends
//! and the admin address handlers run exactly as they do in production.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use kubuno_db::{params, DbPool};
use kubuno_mail::config::Settings;
use kubuno_mail::server::deliver::{self, Disposition, LocalTarget};
use kubuno_mail::server::{config as server_config, queue, store};
use kubuno_mail::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "mail-portability-test-secret";
const PREFIX: &str = "kbmig_";

static EXCLUSIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn migrator() -> kubuno_db::MigratorSet {
    kubuno_db::migrations!(
        "./migrations/postgres",
        "./migrations/mysql",
        "./migrations/sqlite",
    )
}

#[test]
fn migration_set_is_consistent() {
    migrator().check().expect("every flavour variant has a mysql namesake");
}

fn settings_for(engine: &str, url: Option<String>, path: Option<String>) -> kubuno_db::DbSettings {
    serde_json::from_value(json!({
        "engine": engine,
        "url": url,
        "path": path,
        "schema_prefix": PREFIX,
        "max_connections": 4,
        "run_migrations": true,
    }))
    .expect("db settings")
}

/// Connects, empties the scratch namespace and migrates it.
async fn fresh_pool(settings: kubuno_db::DbSettings) -> DbPool {
    let pool = kubuno_db::connect(&settings, kubuno_mail::SCHEMA).await.expect("connect");
    match pool.backend() {
        kubuno_db::Backend::Postgres => {
            pool.execute("DROP SCHEMA IF EXISTS kbmig_mail CASCADE", params![]).await.expect("drop schema");
            pool.execute("CREATE SCHEMA kbmig_mail", params![]).await.expect("create schema");
        }
        kubuno_db::Backend::MySql => {
            pool.execute("DROP DATABASE IF EXISTS kbmig_mail", params![]).await.expect("drop database");
            pool.execute("CREATE DATABASE kbmig_mail", params![]).await.expect("create database");
        }
        kubuno_db::Backend::Sqlite => {}
    }
    migrator().run(&pool, kubuno_mail::SCHEMA).await.expect("migrations");
    pool
}

/// The two internal endpoints of the core the module reads.
async fn fake_core() -> SocketAddr {
    use axum::routing::get;
    let app = axum::Router::new()
        .route(
            "/internal/modules/mail/settings",
            get(|| async {
                axum::Json(json!({ "settings": {
                    "server_domains": ["test.local"],
                    "server_hostname": "mx.test.local",
                    "outbound_enabled": true,
                    "greylisting_enabled": false,
                }}))
            }),
        )
        .route(
            "/internal/domains",
            get(|| async {
                axum::Json(json!({ "domains": [
                    { "name": "test.local", "kind": "primary", "verified": true }
                ]}))
            }),
        )
        .fallback(|| async { (StatusCode::NOT_FOUND, "not served by the fake core") });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

struct Harness {
    state: AppState,
    router: axum::Router,
    engine: &'static str,
}

impl Harness {
    async fn new(pool: DbPool, engine: &'static str) -> Self {
        let core = fake_core().await;
        let mut settings = Settings::load().expect("default settings");
        settings.core.internal_secret = SECRET.to_string();
        settings.core.url = format!("http://{core}");
        settings.mail.encryption_key = "11".repeat(32);
        let state = AppState { db: pool, settings: Arc::new(settings) };
        let router = kubuno_mail::router::build(state.clone());
        Harness { state, router, engine }
    }

    fn db(&self) -> &DbPool {
        &self.state.db
    }

    /// One HTTP request through the real router, as `user` (admin or not).
    async fn call(&self, method: &str, uri: &str, user: Uuid, admin: bool, body: Option<Value>) -> (StatusCode, Value) {
        self.call_with(method, uri, user, admin, body, &[]).await
    }

    async fn call_with(
        &self,
        method: &str,
        uri: &str,
        user: Uuid,
        admin: bool,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let identity = kubuno_modauth::ModuleUser {
            id: user,
            role: if admin { "admin" } else { "user" }.to_string(),
            email: format!("{user}@test.local"),
        };
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(kubuno_modauth::TOKEN_HEADER, kubuno_modauth::sign(SECRET.as_bytes(), &identity, "mail"))
            .header("Content-Type", "application/json");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = req
            .body(match body {
                Some(b) => Body::from(b.to_string()),
                None => Body::empty(),
            })
            .expect("request");
        let resp = self.router.clone().oneshot(req).await.expect("response");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, value)
    }

    /// `call`, asserting a 2xx answer.
    async fn ok(&self, method: &str, uri: &str, user: Uuid, admin: bool, body: Option<Value>) -> Value {
        let (status, value) = self.call(method, uri, user, admin, body).await;
        assert!(status.is_success(), "[{}] {method} {uri} -> {status}: {value}", self.engine);
        value
    }
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array()
        .or_else(|| v.as_object().and_then(|o| o.values().find_map(Value::as_array)))
        .unwrap_or_else(|| panic!("expected an array in {v}"))
}

fn uuid_of(v: &Value, key: &str) -> Uuid {
    v[key].as_str().and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("no uuid `{key}` in {v}"))
}

fn raw_message(from: &str, to: &str, subject: &str, body: &str, message_id: &str, in_reply_to: Option<&str>) -> Vec<u8> {
    let mut s = format!(
        "From: Carol Remote <{from}>\r\nTo: <{to}>\r\nSubject: {subject}\r\nMessage-ID: <{message_id}>\r\n\
         Date: Mon, 05 Oct 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\n"
    );
    if let Some(parent) = in_reply_to {
        s.push_str(&format!("In-Reply-To: <{parent}>\r\nReferences: <{parent}>\r\n"));
    }
    s.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
    s.push_str(body);
    s.push_str("\r\n");
    s.into_bytes()
}

async fn exercise(pool: DbPool, engine: &'static str) {
    let h = Harness::new(pool, engine).await;
    let db = h.db().clone();
    let alice = Uuid::new_v4();
    let bob = Uuid::new_v4();
    let admin = Uuid::new_v4();

    // ── Hosted mailboxes, through the admin API (served domain from the fake core)
    let mb = h
        .ok("POST", "/admin/mailboxes", admin, true,
            Some(json!({ "address": "alice@test.local", "user_id": alice, "display_name": "Alice" })))
        .await;
    let alice_mailbox = uuid_of(mb.get("mailbox").unwrap_or(&mb), "id");
    h.ok("POST", "/admin/mailboxes", admin, true,
         Some(json!({ "address": "bob@test.local", "user_id": bob, "display_name": "Bob" })))
        .await;
    let listed = h.ok("GET", "/admin/mailboxes?limit=10", admin, true, None).await;
    assert!(listed.to_string().contains("bob@test.local"), "[{engine}] mailbox list: {listed}");
    h.ok("PATCH", &format!("/admin/mailboxes/{alice_mailbox}"), admin, true,
         Some(json!({ "display_name": "Alice A." })))
        .await;

    // An address is a mailbox, an alias OR a list: the uniqueness trigger.
    let (st, body) = h
        .call("POST", "/admin/aliases", admin, true,
              Some(json!({ "address": "alice@test.local", "destinations": ["bob@test.local"] })))
        .await;
    assert!(st.is_client_error(), "[{engine}] alias over a mailbox must be refused: {st} {body}");
    let direct = db
        .execute(
            "INSERT INTO mail.aliases (id, address, domain, destinations) VALUES ($1, $2, $3, $4)",
            params![Uuid::new_v4(), "alice@test.local", "test.local", vec!["x@test.local".to_string()]],
        )
        .await;
    assert!(direct.is_err(), "[{engine}] the address trigger must refuse a duplicate address");

    // Aliases and lists (JSON array columns).
    let alias = h
        .ok("POST", "/admin/aliases", admin, true,
            Some(json!({ "address": "team@test.local", "destinations": ["alice@test.local", "bob@test.local"] })))
        .await;
    let alias_id = uuid_of(alias.get("alias").unwrap_or(&alias), "id");
    let got = h.ok("GET", &format!("/admin/aliases/{alias_id}"), admin, true, None).await;
    assert!(got.to_string().contains("bob@test.local"), "[{engine}] alias destinations: {got}");
    h.ok("PATCH", &format!("/admin/aliases/{alias_id}"), admin, true,
         Some(json!({ "destinations": ["alice@test.local"] })))
        .await;
    let list = h
        .ok("POST", "/admin/mailing-lists", admin, true,
            Some(json!({ "address": "all@test.local", "name": "All", "post_policy": "allowed",
                         "allowed_senders": ["carol@remote.example"], "members": ["alice@test.local"] })))
        .await;
    let list_id = uuid_of(list.get("list").unwrap_or(&list), "id");
    h.ok("POST", &format!("/admin/mailing-lists/{list_id}/members"), admin, true,
         Some(json!({ "addresses": ["bob@test.local"] })))
        .await;
    h.ok("GET", &format!("/admin/mailing-lists/{list_id}"), admin, true, None).await;
    h.ok("GET", "/admin/mailing-lists", admin, true, None).await;
    h.ok("GET", "/admin/aliases", admin, true, None).await;
    h.ok("PUT", "/admin/domains/test.local", admin, true,
         Some(json!({ "default_quota_bytes": 0, "max_mailboxes": 50 })))
        .await;
    h.ok("GET", "/admin/domains", admin, true, None).await;
    h.ok("GET", "/admin/relay", admin, true, None).await;
    h.ok("PUT", "/admin/relay", admin, true,
         Some(json!({ "enabled": false, "host": "relay.test.local", "port": 25, "security": "none" })))
        .await;

    // ── The local accounts the mailboxes created
    let accounts = h.ok("GET", "/accounts", alice, false, None).await;
    let account = arr(&accounts).first().cloned().expect("alice has her local account");
    assert_eq!(account["kind"], "local", "[{engine}] {account}");
    let account_id = uuid_of(&account, "id");
    let labels = h.ok("GET", "/labels", alice, false, None).await;
    assert!(!arr(&labels).is_empty(), "[{engine}] system labels created with the account");

    // ── Incoming mail, through the real delivery path
    let cfg = server_config::fetch(&reqwest::Client::new(), &h.state.settings)
        .await
        .expect("server configuration from the fake core");
    let attachments = std::env::temp_dir().join(format!("kbmig-mail-att-{}", Uuid::new_v4()));
    let target = LocalTarget { user_id: alice, account_id };
    let first = deliver::deliver_local(
        &db, &cfg, "carol@remote.example", "alice@test.local", target,
        &raw_message("carol@remote.example", "alice@test.local", "Réunion trimestrielle",
                     "Bonjour Alice, voici l'ordre du jour du budget.", "m1@remote.example", None),
        &attachments.to_string_lossy(), Disposition::Inbox, Some("tls"), Some("pass"),
    )
    .await
    .unwrap_or_else(|e| panic!("[{engine}] first delivery: {e:#}"));
    let reply = deliver::deliver_local(
        &db, &cfg, "carol@remote.example", "alice@test.local", target,
        &raw_message("carol@remote.example", "alice@test.local", "Re: Réunion trimestrielle",
                     "Petite précision sur le budget.", "m2@remote.example", Some("m1@remote.example")),
        &attachments.to_string_lossy(), Disposition::Inbox, None, None,
    )
    .await
    .unwrap_or_else(|e| panic!("[{engine}] reply delivery: {e:#}"));
    deliver::deliver_local(
        &db, &cfg, "news@shop.example", "alice@test.local", target,
        &raw_message("news@shop.example", "alice@test.local", "Promo de la semaine",
                     "Tout doit disparaitre", "m3@shop.example", None),
        &attachments.to_string_lossy(), Disposition::Spam, None, None,
    )
    .await
    .unwrap_or_else(|e| panic!("[{engine}] spam delivery: {e:#}"));

    // ── Reading
    let inbox = h.ok("GET", "/threads?folder=inbox", alice, false, None).await;
    let threads = arr(&inbox);
    assert_eq!(threads.len(), 1, "[{engine}] the reply joins the first thread: {inbox}");
    let thread_id = uuid_of(&threads[0], "id");
    let thread = h.ok("GET", &format!("/threads/{thread_id}"), alice, false, None).await;
    assert!(thread.to_string().contains("Petite précision"), "[{engine}] thread: {thread}");
    let msg = h.ok("GET", &format!("/messages/{first}"), alice, false, None).await;
    assert!(msg.to_string().contains("ordre du jour"), "[{engine}] message: {msg}");
    h.ok("PATCH", &format!("/messages/{reply}/read"), alice, false, Some(json!({ "is_read": true }))).await;
    h.ok("POST", &format!("/messages/{first}/star"), alice, false, None).await;
    h.ok("GET", "/counts", alice, false, None).await;
    h.ok("GET", "/changes?since=0", alice, false, None).await;
    h.ok("GET", "/folders", alice, false, None).await;
    h.ok("GET", "/subscriptions", alice, false, None).await;
    h.ok("GET", "/threads?folder=spam", alice, false, None).await;
    h.ok("GET", "/threads?starred=true", alice, false, None).await;
    h.ok("GET", "/threads?unread=true", alice, false, None).await;
    h.ok("GET", "/threads?category=main", alice, false, None).await;
    let sugg = h.ok("GET", "/addresses?q=car", alice, false, None).await;
    assert!(sugg.to_string().contains("carol@remote.example"), "[{engine}] suggestions: {sugg}");

    // ── Search (the Gmail-style compiler)
    for (q, expect_hit) in [
        ("from:carol", true),
        ("budget", true),
        ("trimestrielle", true),
        ("is:read", true),
        ("is:starred", false),
        ("subject:réunion", true),
        ("-budget", false),
        ("in:anywhere promo", true),
        ("after:2020/01/01 before:2100/01/01 carol", true),
        ("larger:1 has:nouserlabels", true),
        ("+budget", true),
        ("zzzzunfindable", false),
    ] {
        let uri = format!("/threads?search={}", urlencode(q));
        let res = h.ok("GET", &uri, alice, false, None).await;
        let hit = !arr(&res).is_empty();
        assert_eq!(hit, expect_hit, "[{engine}] search `{q}` -> {res}");
    }

    // ── Labels on a thread
    let label = h
        .ok("POST", "/labels", alice, false, Some(json!({ "account_id": account_id, "name": "Projets", "color": "#336699" })))
        .await;
    let label_id = uuid_of(&label, "id");
    h.ok("PATCH", &format!("/labels/{label_id}"), alice, false, Some(json!({ "list_visibility": "unread" }))).await;
    h.ok("POST", &format!("/threads/{thread_id}/labels/{label_id}"), alice, false, None).await;
    let by_label = h.ok("GET", &format!("/threads?label_id={label_id}"), alice, false, None).await;
    assert_eq!(arr(&by_label).len(), 1, "[{engine}] thread by label: {by_label}");
    let res = h.ok("GET", &format!("/threads?search={}", urlencode("label:projets")), alice, false, None).await;
    assert_eq!(arr(&res).len(), 1, "[{engine}] label: search: {res}");
    h.ok("DELETE", &format!("/threads/{thread_id}/labels/{label_id}"), alice, false, None).await;

    // ── Thread state, and what the IMAP store sees of it (modseq / tombstones)
    let modseq_before = store::highest_modseq(&db, alice, "inbox").await.expect("highest modseq");
    h.ok("POST", &format!("/threads/{thread_id}/star"), alice, false, None).await;
    h.ok("POST", &format!("/threads/{thread_id}/important"), alice, false, None).await;
    h.ok("POST", &format!("/threads/{thread_id}/mute"), alice, false, None).await;
    h.ok("POST", &format!("/threads/{thread_id}/mute"), alice, false, None).await;
    h.ok("POST", &format!("/threads/{thread_id}/read"), alice, false, Some(json!({ "is_read": false }))).await;
    h.ok("POST", &format!("/threads/{thread_id}/category"), alice, false, Some(json!({ "category": "social" }))).await;
    let snooze_until = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    h.ok("POST", &format!("/threads/{thread_id}/snooze"), alice, false, Some(json!({ "until": snooze_until }))).await;
    h.ok("GET", "/threads?snoozed=true", alice, false, None).await;
    h.ok("POST", &format!("/threads/{thread_id}/snooze"), alice, false, Some(json!({ "until": null }))).await;
    let modseq_after = store::highest_modseq(&db, alice, "inbox").await.expect("highest modseq");
    assert!(modseq_after > modseq_before, "[{engine}] every change advances modseq ({modseq_before} -> {modseq_after})");

    let inbox_uids = store::list(&db, alice, "inbox").await.expect("store list");
    assert_eq!(inbox_uids.len(), 2, "[{engine}] IMAP view of the inbox");
    h.ok("POST", &format!("/threads/{thread_id}/move"), alice, false, Some(json!({ "folder": "archive" }))).await;
    let gone = store::vanished_since(&db, alice, "inbox", modseq_after).await.expect("vanished");
    assert_eq!(gone.len(), 2, "[{engine}] QRESYNC tombstones after the move: {gone:?}");
    h.ok("POST", &format!("/threads/{thread_id}/move"), alice, false, Some(json!({ "folder": "inbox" }))).await;
    let gone = store::vanished_since(&db, alice, "inbox", modseq_after).await.expect("vanished");
    assert!(gone.is_empty(), "[{engine}] arriving back clears the tombstones: {gone:?}");

    // IMAP-side operations on one message.
    let one = inbox_uids[0].id;
    store::set_read(&db, alice, one, true).await.expect("set_read");
    store::set_starred(&db, alice, one, false).await.expect("set_starred");
    let copied = store::copy_to(&db, alice, one, "archive").await.expect("copy_to");
    assert!(copied.is_some(), "[{engine}] COPY returns the new local uid");
    let moved = store::move_to(&db, alice, one, "trash").await.expect("move_to");
    assert!(moved.is_some(), "[{engine}] MOVE returns the local uid");
    store::move_to(&db, alice, one, "inbox").await.expect("move back");
    let appended = store::append(&db, alice, "drafts", &raw_message("alice@test.local", "bob@test.local", "Brouillon IMAP", "corps", "a1@test.local", None), true, false)
        .await
        .expect("append");
    assert!(appended > 0, "[{engine}] APPEND returns a local uid");

    // ── Drafts
    let draft = h
        .ok("POST", "/drafts", alice, false,
            Some(json!({ "account_id": account_id, "subject": "Brouillon", "body_html": "<p>x</p>",
                         "to_addresses": [{ "email": "bob@test.local", "name": "Bob" }] })))
        .await;
    let draft_id = uuid_of(&draft, "id");
    h.ok("PATCH", &format!("/drafts/{draft_id}"), alice, false,
         Some(json!({ "account_id": account_id, "subject": "Brouillon 2", "body_html": "<p>y</p>" })))
        .await;
    h.ok("GET", "/drafts", alice, false, None).await;
    h.ok("GET", "/scheduled", alice, false, None).await;
    h.ok("DELETE", &format!("/drafts/{draft_id}"), alice, false, None).await;

    // ── Filters, blocked senders, image senders
    let filter = h
        .ok("POST", "/filters", alice, false,
            Some(json!({ "from_contains": "carol", "act_star": true, "act_label_id": label_id, "apply_existing": true })))
        .await;
    let filter_id = uuid_of(&filter, "id");
    h.ok("GET", "/filters", alice, false, None).await;
    h.ok("DELETE", &format!("/filters/{filter_id}"), alice, false, None).await;
    h.ok("POST", "/blocked", alice, false, Some(json!({ "email": "spammer@bad.example" }))).await;
    h.ok("POST", "/blocked", alice, false, Some(json!({ "email": "spammer@bad.example" }))).await;
    let blocked = h.ok("GET", "/blocked", alice, false, None).await;
    assert_eq!(arr(&blocked).len(), 1, "[{engine}] blocking twice is idempotent: {blocked}");
    h.ok("DELETE", &format!("/blocked/{}", uuid_of(&arr(&blocked)[0], "id")), alice, false, None).await;
    h.ok("POST", "/image-senders", alice, false, Some(json!({ "email": "@shop.example" }))).await;
    let img = h.ok("GET", "/image-senders", alice, false, None).await;
    h.ok("DELETE", &format!("/image-senders/{}", uuid_of(&img["senders"][0], "id")), alice, false, None).await;

    // ── Settings stores
    let tpl = h
        .ok("POST", "/templates", alice, false, Some(json!({ "name": "Merci", "subject": "Merci", "body_html": "<p>Merci</p>" })))
        .await;
    let (st, _) = h
        .call("POST", "/templates", alice, false, Some(json!({ "name": "MERCI", "subject": "x", "body_html": "" })))
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "[{engine}] template names are unique case-insensitively");
    h.ok("PUT", &format!("/templates/{}", uuid_of(&tpl, "id")), alice, false,
         Some(json!({ "name": "Merci !", "subject": "Merci", "body_html": "<p>Merci beaucoup</p>" })))
        .await;
    h.ok("GET", "/templates", alice, false, None).await;
    let grp = h
        .ok("POST", "/recipient-groups", alice, false,
            Some(json!({ "name": "Équipe", "members": [{ "email": "bob@test.local", "name": "Bob" }, { "email": "carol@remote.example" }] })))
        .await;
    let groups = h.ok("GET", "/recipient-groups", alice, false, None).await;
    assert!(groups.to_string().contains("carol@remote.example"), "[{engine}] group members (JSON): {groups}");
    h.ok("DELETE", &format!("/recipient-groups/{}", uuid_of(&grp, "id")), alice, false, None).await;
    h.ok("PUT", "/vacation", alice, false,
         Some(json!({ "enabled": true, "startDate": "2026-01-01", "endDate": "2099-12-31",
                      "subject": "Absent", "messageHtml": "<p>Absent</p>", "contactsOnly": false })))
        .await;
    let vac = h.ok("GET", "/vacation", alice, false, None).await;
    assert_eq!(vac["endDate"], "2099-12-31", "[{engine}] vacation dates: {vac}");
    h.ok("PUT", "/forwarding", alice, false,
         Some(json!({ "forwardAddresses": [{ "email": "bob@test.local", "enabled": true }], "forwardKeep": true, "forwardingAllowed": true })))
        .await;
    h.ok("GET", "/forwarding", alice, false, None).await;
    h.ok("PUT", "/forwarding", alice, false,
         Some(json!({ "forwardAddresses": [], "forwardKeep": true, "forwardingAllowed": true })))
        .await;
    let popimap = h.ok("GET", "/pop-imap", alice, false, None).await;
    h.ok("PUT", "/pop-imap", alice, false, Some(popimap)).await;
    h.ok("PATCH", "/spam/settings", alice, false, Some(json!({ "auto_classify": true, "threshold": 0.9 }))).await;
    h.ok("POST", "/spam/train", alice, false, None).await;
    let stats = h.ok("GET", "/spam/stats", alice, false, None).await;
    let threshold = stats["threshold"].as_f64().unwrap_or_default();
    assert!((threshold - 0.9).abs() < 1e-4, "[{engine}] spam threshold (f32): {stats}");
    h.ok("GET", "/send-as", alice, false, None).await;
    h.ok("GET", "/delegations", alice, false, None).await;
    h.ok("GET", "/delegations/incoming", alice, false, None).await;
    h.ok("GET", "/mailbox-credentials", alice, false, None).await;
    h.ok("POST", "/mailbox-credentials", alice, false, Some(json!({ "username": "alice@test.local", "label": "Thunderbird" }))).await;
    h.ok("GET", "/dkim", admin, true, None).await;
    h.ok("GET", "/pgp/status", alice, false, None).await;

    // ── Sending from a hosted mailbox to another local address (+ idempotency)
    let send = json!({ "account_id": account_id, "to_addresses": [{ "email": "bob@test.local", "name": "Bob" }],
                       "subject": "Bonjour Bob", "body_html": "<p>Salut Bob</p>" });
    let (st, first_send) = h
        .call_with("POST", "/send", alice, false, Some(send.clone()), &[("Idempotency-Key", "k-1")])
        .await;
    assert!(st.is_success(), "[{engine}] local send: {st} {first_send}");
    let (st, replay) = h
        .call_with("POST", "/send", alice, false, Some(send), &[("Idempotency-Key", "k-1")])
        .await;
    assert!(st.is_success(), "[{engine}] idempotent replay: {st} {replay}");
    let bob_inbox = h.ok("GET", "/threads?folder=inbox", bob, false, None).await;
    assert_eq!(arr(&bob_inbox).len(), 1, "[{engine}] bob got exactly one copy: {bob_inbox}");
    let sent = h.ok("GET", "/threads?folder=sent", alice, false, None).await;
    assert!(!arr(&sent).is_empty(), "[{engine}] alice's sent copy: {sent}");

    // ── The outbound queue (claim with the engine's locking, then settle)
    let qid = queue::enqueue(&db, Some(alice), Some(account_id), "alice@test.local", b"Subject: x\r\n\r\ny\r\n", false,
                             &[("dave@far.example".to_string(), "far.example".to_string())], 5)
        .await
        .expect("enqueue");
    let claimed = queue::claim_batch(&db, Uuid::new_v4(), std::time::Duration::from_secs(60), 10)
        .await
        .expect("claim");
    assert!(claimed.iter().any(|c| c.message_id == qid), "[{engine}] the queued recipient is claimable");
    for c in &claimed {
        queue::mark_sent(&db, c.recipient_id).await.expect("mark_sent");
    }
    assert!(queue::claim_batch(&db, Uuid::new_v4(), std::time::Duration::from_secs(60), 10).await.expect("claim").is_empty(),
            "[{engine}] nothing left to claim");

    // ── Deleting
    h.ok("DELETE", &format!("/messages/{reply}"), alice, false, None).await;
    h.ok("DELETE", &format!("/threads/{thread_id}"), alice, false, None).await;
    h.ok("DELETE", &format!("/labels/{label_id}"), alice, false, None).await;
    h.ok("DELETE", &format!("/admin/mailing-lists/{list_id}"), admin, true, None).await;
    h.ok("DELETE", &format!("/admin/aliases/{alias_id}"), admin, true, None).await;
    h.ok("DELETE", &format!("/admin/mailboxes/{alice_mailbox}"), admin, true, None).await;

    let _ = std::fs::remove_dir_all(&attachments);
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sqlite_end_to_end() {
    let _g = EXCLUSIVE.lock().await;
    let dir = std::env::temp_dir().join(format!("kbmig-mail-{}", Uuid::new_v4()));
    let pool = fresh_pool(settings_for("sqlite", None, Some(dir.to_string_lossy().into_owned()))).await;
    exercise(pool, "sqlite").await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_end_to_end() {
    let Ok(url) = std::env::var("KUBUNO_PG_TEST_URL") else {
        eprintln!("skipping: KUBUNO_PG_TEST_URL not set");
        return;
    };
    let _g = EXCLUSIVE.lock().await;
    exercise(fresh_pool(settings_for("postgres", Some(url), None)).await, "postgres").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mysql_end_to_end() {
    let Ok(url) = std::env::var("KUBUNO_MYSQL_TEST_URL") else {
        eprintln!("skipping: KUBUNO_MYSQL_TEST_URL not set");
        return;
    };
    let _g = EXCLUSIVE.lock().await;
    exercise(fresh_pool(settings_for("mysql", Some(url), None)).await, "mysql").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mariadb_end_to_end() {
    let Ok(url) = std::env::var("KUBUNO_MARIADB_TEST_URL") else {
        eprintln!("skipping: KUBUNO_MARIADB_TEST_URL not set");
        return;
    };
    let _g = EXCLUSIVE.lock().await;
    exercise(fresh_pool(settings_for("mysql", Some(url), None)).await, "mariadb").await;
}
