package com.kubuno.mail.data

/**
 * The four tabs of the web's mobile bottom bar. [key] scopes the cached rows
 * in Room and, for the thread-based folders, is the server `folder` value.
 */
enum class MailFolder(val key: String, val label: String) {
    INBOX("inbox", "Réception"),
    STARRED("starred", "Suivis"),
    SENT("sent", "Envoyés"),
    DRAFTS("drafts", "Brouillons"),
}
