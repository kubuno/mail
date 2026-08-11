use axum::{
    routing::{delete, get, patch, post, put},
    Router,
};
use tower_http::cors::{Any, CorsLayer};

use crate::{
    handlers::{
        accounts, avatar,
        addresses::{aliases, directory, domains, lists, mailboxes},
        delegation,
        diagnostics, dkim, drafts, filters, folders, forwarding, labels, mailbox, messages, oauth, pgp,
        pop_imap, recipient_groups, relay,
        send_as, spam, templates, threads, vacation, wkd,
    },
    state::AppState,
};

pub fn build(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Accounts
        .route("/accounts",              get(accounts::list_accounts).post(accounts::create_account))
        .route("/accounts/test",         post(accounts::test_connection))
        .route("/accounts/:id",          get(accounts::get_account).patch(accounts::update_account).delete(accounts::delete_account))
        .route("/accounts/:id/test",     post(accounts::test_existing_account))
        .route("/accounts/:id/sync",     post(accounts::trigger_sync))
        // OpenPGP / GPG — key management (Wave 1)
        .route("/pgp/status",            get(pgp::status))
        .route("/pgp/keys",              get(pgp::list_keys))
        .route("/pgp/keys/generate",     post(pgp::generate_key))
        .route("/pgp/keys/import",       post(pgp::import_key))
        .route("/pgp/keys/:id",          delete(pgp::delete_key))
        .route("/pgp/contacts",          get(pgp::list_contacts).post(pgp::add_contact))
        .route("/pgp/contacts/:id",      delete(pgp::delete_contact))
        // Web Key Directory — serving OUR local mailboxes' public keys. A real
        // WKD client hits the root `/.well-known/openpgpkey/...`, which the core
        // does not proxy to modules; these are reachable at `/api/v1/mail/wkd/...`
        // and become the WKD endpoint once the root path is routed here (see the
        // module report). Public + unauthenticated by design.
        .route("/wkd/policy",            get(wkd::policy))
        .route("/wkd/hu/:hash",          get(wkd::hu))
        // OAuth2 (Gmail / Microsoft) account connection
        .route("/oauth/providers",           get(oauth::providers))
        .route("/oauth/:provider/start",     post(oauth::start))
        .route("/oauth/:provider/callback",  get(oauth::callback))
        // Threads
        .route("/threads",               get(threads::list_threads))
        // Delta sync for offline-first clients: only what changed since a cursor.
        .route("/changes",               get(threads::changes))
        .route("/counts",                get(threads::counts))
        .route("/folders",               get(folders::list_custom_folders))
        // Credentials for the SMTP/IMAP/POP3 services this instance offers.
        .route("/mailbox-credentials",     get(mailbox::list_credentials).post(mailbox::create_credential))
        .route("/mailbox-credentials/:id", delete(mailbox::delete_credential))
        // DKIM signing keys (admin) : generate/list/revoke + the DNS record.
        // Keyed by id, not by domain: a domain holds several keys during a
        // rotation (the one that signs, and the one whose record is propagating).
        .route("/dkim",              get(dkim::list_keys).post(dkim::create_key))
        .route("/dkim/:id",          delete(dkim::delete_key))
        .route("/dkim/:id/activate", post(dkim::activate_key))
        // Deliverability report (admin) : MX, SPF, DKIM, DMARC, PTR, certificat.
        .route("/diagnostics",       get(diagnostics::report))
        // Relais sortant (smarthost, admin) : livrer via UN serveur SMTP plutôt
        // que direct au MX. Mot de passe write-only (jamais renvoyé par GET).
        .route("/admin/relay",       get(relay::get_relay).put(relay::put_relay))
        // ── Adresses locales (admin) ────────────────────────────────────────
        // Sous `/admin/*` et non `/addresses` : ce dernier est déjà la
        // complétion de destinataires du composeur (messages::suggest_addresses),
        // et une adresse d'administration n'est pas une suggestion de saisie.
        // Les comptes auxquels une boîte peut être rattachée. Relayé vers
        // le core : le module ne peut pas lire le schéma `core`.
        .route("/admin/directory/users",            get(directory::list_users))
        .route("/admin/mailboxes",                  get(mailboxes::list_mailboxes).post(mailboxes::create_mailbox))
        .route("/admin/mailboxes/:id",              get(mailboxes::get_mailbox).patch(mailboxes::update_mailbox).delete(mailboxes::delete_mailbox))
        // Génère (ou renouvelle) l'identifiant IMAP/SMTP de la boîte : le mot de
        // passe n'est renvoyé qu'ici, une seule fois.
        .route("/admin/mailboxes/:id/credential",   post(mailboxes::issue_mailbox_credential))
        .route("/admin/aliases",                    get(aliases::list_aliases).post(aliases::create_alias))
        .route("/admin/aliases/:id",                get(aliases::get_alias).patch(aliases::update_alias).delete(aliases::delete_alias))
        .route("/admin/mailing-lists",              get(lists::list_mailing_lists).post(lists::create_mailing_list))
        .route("/admin/mailing-lists/:id",          get(lists::get_mailing_list).patch(lists::update_mailing_list).delete(lists::delete_mailing_list))
        // PUT remplace toute l'appartenance (idempotent) ; POST/DELETE ajoutent
        // ou retirent un lot d'adresses.
        .route("/admin/mailing-lists/:id/members",  put(lists::set_members).post(lists::add_members).delete(lists::remove_members))
        .route("/admin/domains",                    get(domains::list_domains))
        .route("/admin/domains/:domain",            put(domains::upsert_domain_policy).delete(domains::delete_domain_policy))
        .route("/threads/:id",           get(threads::get_thread).delete(threads::delete_thread))
        .route("/threads/:id/star",      post(threads::star_thread))
        .route("/threads/:id/important", post(threads::important_thread))
        .route("/threads/:id/snooze",    post(threads::snooze_thread))
        .route("/threads/:id/read",      post(threads::read_thread))
        .route("/threads/:id/mute",      post(threads::mute_thread))
        .route("/threads/:id/move",      post(threads::move_thread))
        .route("/threads/:id/category",  post(threads::set_thread_category))
        .route("/threads/:id/labels/:label_id", post(labels::add_thread_label).delete(labels::remove_thread_label))
        .route("/subscriptions",         get(threads::subscriptions))
        // Messages
        .route("/messages/:id",          get(messages::get_message).delete(messages::delete_message))
        .route("/messages/:id/attachments/:index", get(messages::download_attachment))
        .route("/messages/:id/star",     post(messages::star_message))
        .route("/messages/:id/read",     patch(messages::mark_read))
        .route("/send",                  post(messages::send_message))
        // Sender avatar (BIMI, resolved and cached server-side).
        .route("/avatar",                get(avatar::sender_avatar))
        .route("/addresses",             get(messages::suggest_addresses))
        // Drafts
        .route("/drafts",                get(drafts::list_drafts).post(drafts::save_draft))
        .route("/scheduled",             get(drafts::scheduled_drafts))
        .route("/drafts/:id",            patch(drafts::update_draft).delete(drafts::delete_draft))
        // Labels
        .route("/labels",                get(labels::list_labels).post(labels::create_label))
        .route("/labels/:id",            patch(labels::update_label).delete(labels::delete_label))
        // Filtres / règles automatiques
        .route("/filters",               get(filters::list_filters).post(filters::create_filter))
        .route("/filters/:id",           delete(filters::delete_filter))
        // Adresses bloquées
        .route("/blocked",               get(filters::list_blocked).post(filters::block_sender))
        .route("/blocked/:id",           delete(filters::unblock_sender))
        // Répondeur d'absence (réponse automatique)
        .route("/vacation",              get(vacation::get_vacation).put(vacation::put_vacation))
        // Délégation d'accès au compte (façon Gmail) : un mandant accorde à un
        // délégué l'accès à sa boîte (lecture + envoi « au nom de »).
        .route("/delegations",              get(delegation::list_granted).post(delegation::grant))
        .route("/delegations/incoming",     get(delegation::list_incoming))
        .route("/delegations/:id",          delete(delegation::revoke))
        .route("/delegations/:id/accept",   post(delegation::accept))
        .route("/delegations/:id/decline",  post(delegation::decline))
        // « Envoyer en tant que » : adresses expéditeur vérifiées par propriété.
        .route("/send-as",               get(send_as::list).post(send_as::add))
        .route("/send-as/:id",           delete(send_as::remove))
        .route("/send-as/:id/resend",    post(send_as::resend))
        .route("/send-as/:id/verify",    post(send_as::verify))
        // Modèles d'e-mail réutilisables (menu « Nouveau message »).
        .route("/templates",             get(templates::list).post(templates::create))
        .route("/templates/:id",         put(templates::update).delete(templates::remove))
        // Groupes personnels de destinataires (listes de diffusion perso).
        .route("/recipient-groups",      get(recipient_groups::list).post(recipient_groups::create))
        .route("/recipient-groups/:id",  put(recipient_groups::update).delete(recipient_groups::remove))
        // Transfert automatique des messages entrants
        .route("/forwarding",            get(forwarding::get_forwarding).put(forwarding::put_forwarding))
        .route("/pop-imap",              get(pop_imap::get_pop_imap).put(pop_imap::put_pop_imap))
        // Anti-spam bayésien
        .route("/spam/stats",            get(spam::stats))
        .route("/spam/settings",         patch(spam::update_settings))
        .route("/spam/train",            post(spam::train))
        // Settings (renvoie vers la page settings du frontend)
        .route("/settings",              get(|| async { axum::Json(serde_json::json!({ "module": "mail" })) }))
        .layer(cors)
        .with_state(state)
}
