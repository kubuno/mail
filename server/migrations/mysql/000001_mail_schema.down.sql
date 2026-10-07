-- Drops everything 000001 created, children before parents.
DROP VIEW IF EXISTS mail.dkim_keys;
DROP TABLE IF EXISTS mail.image_allowed_senders, mail.recipient_groups, mail.email_templates,
    mail.pop_imap_settings, mail.delegations, mail.send_idempotency, mail.send_as_addresses,
    mail.forwarding_rules, mail.vacation_sent, mail.vacation_responders, mail.pgp_contacts,
    mail.pgp_keys, mail.sender_avatars, mail.outbound_relay, mail.greylist, mail.dkim_keys_all,
    mail.outbound_recipients, mail.outbound_messages, mail.server_sessions,
    mail.mailbox_credentials, mail.folder_sync, mail.oauth_states, mail.address_index,
    mail.spam_stats, mail.spam_tokens, mail.blocked_senders, mail.filters,
    mail.message_tombstones, mail.thread_labels, mail.drafts, mail.messages, mail.threads,
    mail.labels, mail.accounts, mail.domain_policies, mail.mailing_list_members,
    mail.mailing_lists, mail.aliases, mail.mailboxes, mail.change_counter;
