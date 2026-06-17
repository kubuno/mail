//! Naive Bayes spam classifier (per user).
//!
//! Implements the token-combining scheme from Paul Graham's "A Plan for Spam":
//! every message is reduced to a set of distinct tokens; each token gets a spam
//! probability from the training corpus; the message score combines the most
//! informative tokens. The model lives in `mail.spam_tokens` / `mail.spam_stats`
//! and is scoped by `user_id`, so each owner has a personal classifier.

use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

/// Tokens shorter/longer than these bounds carry little signal and are dropped.
const MIN_TOKEN_LEN: usize = 2;
const MAX_TOKEN_LEN: usize = 30;
/// Number of most-informative tokens combined into the final score.
const TOP_N: usize = 15;
/// Probability assigned to a token never seen during training (Graham's 0.4).
const UNKNOWN_PROB: f64 = 0.4;
/// A token must have at least this weighted count to be trusted.
const MIN_TOKEN_WEIGHT: f64 = 5.0;
/// Ham counts are weighted ×2 to bias against false positives (Graham).
const HAM_WEIGHT: f64 = 2.0;
/// Below this many trained messages we refuse to auto-classify (too noisy).
const MIN_TRAIN_MESSAGES: i64 = 20;

/// Reduce a message to its distinct lowercased tokens.
///
/// Presence is binary per message (a word repeated in one mail counts once),
/// which is the standard bag-of-tokens used by Bayesian spam filters. The
/// sender address and its domain are kept as dedicated tokens because they are
/// strong signals.
pub fn tokenize(subject: &str, body: Option<&str>, from_email: &str) -> HashSet<String> {
    let mut set = HashSet::new();

    let from = from_email.trim().to_lowercase();
    if !from.is_empty() {
        set.insert(format!("from:{from}"));
        if let Some((_, domain)) = from.split_once('@') {
            if !domain.is_empty() {
                set.insert(format!("fromdom:{domain}"));
            }
        }
    }

    for text in [Some(subject), body].into_iter().flatten() {
        for raw in text.split(|c: char| !c.is_alphanumeric()) {
            if raw.is_empty() {
                continue;
            }
            let tok = raw.to_lowercase();
            let len = tok.chars().count();
            // Keep alphanumeric tokens of reasonable length; drop pure numbers
            // (dates, ids) which add noise without discriminating spam.
            if (MIN_TOKEN_LEN..=MAX_TOKEN_LEN).contains(&len)
                && !tok.chars().all(|c| c.is_numeric())
            {
                set.insert(tok);
            }
        }
    }

    set
}

/// Add the token set to the corpus under the given class.
pub async fn train(
    db: &PgPool,
    user_id: Uuid,
    tokens: &HashSet<String>,
    is_spam: bool,
) -> anyhow::Result<()> {
    if tokens.is_empty() {
        return Ok(());
    }
    let (s_inc, h_inc): (i32, i32) = if is_spam { (1, 0) } else { (0, 1) };
    let toks: Vec<String> = tokens.iter().cloned().collect();

    let mut tx = db.begin().await?;

    // Batch upsert all tokens of the message in one statement via UNNEST.
    sqlx::query(
        r#"INSERT INTO mail.spam_tokens (user_id, token, spam_count, ham_count)
           SELECT $1, t, $3, $4 FROM UNNEST($2::text[]) AS t
           ON CONFLICT (user_id, token) DO UPDATE
             SET spam_count = mail.spam_tokens.spam_count + EXCLUDED.spam_count,
                 ham_count  = mail.spam_tokens.ham_count  + EXCLUDED.ham_count"#,
    )
    .bind(user_id)
    .bind(&toks)
    .bind(s_inc)
    .bind(h_inc)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO mail.spam_stats (user_id, spam_messages, ham_messages)
           VALUES ($1, $2, $3)
           ON CONFLICT (user_id) DO UPDATE
             SET spam_messages = mail.spam_stats.spam_messages + $2,
                 ham_messages  = mail.spam_stats.ham_messages  + $3,
                 updated_at    = NOW()"#,
    )
    .bind(user_id)
    .bind(s_inc)
    .bind(h_inc)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Remove the token set from the corpus under the given class (floored at 0).
/// Used when a message is re-classified to undo its previous contribution.
pub async fn untrain(
    db: &PgPool,
    user_id: Uuid,
    tokens: &HashSet<String>,
    was_spam: bool,
) -> anyhow::Result<()> {
    if tokens.is_empty() {
        return Ok(());
    }
    let (s_dec, h_dec): (i32, i32) = if was_spam { (1, 0) } else { (0, 1) };
    let toks: Vec<String> = tokens.iter().cloned().collect();

    let mut tx = db.begin().await?;

    sqlx::query(
        r#"UPDATE mail.spam_tokens
           SET spam_count = GREATEST(0, spam_count - $3),
               ham_count  = GREATEST(0, ham_count  - $4)
           WHERE user_id = $1 AND token = ANY($2)"#,
    )
    .bind(user_id)
    .bind(&toks)
    .bind(s_dec)
    .bind(h_dec)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"UPDATE mail.spam_stats
           SET spam_messages = GREATEST(0, spam_messages - $2),
               ham_messages  = GREATEST(0, ham_messages  - $3),
               updated_at    = NOW()
           WHERE user_id = $1"#,
    )
    .bind(user_id)
    .bind(s_dec)
    .bind(h_dec)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Train (or re-train) a single message towards `is_spam`, idempotently.
///
/// `prev` is the message's current `spam_trained` guard (None / Some(0=ham) /
/// Some(1=spam)). Returns the new guard value to persist on the message row.
/// Re-classifying first undoes the previous contribution, so counts never drift.
pub async fn learn_message(
    db: &PgPool,
    user_id: Uuid,
    subject: &str,
    body: Option<&str>,
    from_email: &str,
    is_spam: bool,
    prev: Option<i16>,
) -> anyhow::Result<i16> {
    let target: i16 = if is_spam { 1 } else { 0 };
    if prev == Some(target) {
        return Ok(target); // already trained this way, nothing to do
    }
    let tokens = tokenize(subject, body, from_email);
    if let Some(p) = prev {
        untrain(db, user_id, &tokens, p == 1).await?;
    }
    train(db, user_id, &tokens, is_spam).await?;
    Ok(target)
}

/// Compute the spam probability of a message in [0, 1].
///
/// Returns `None` when the model is too small to be trusted (so callers fall
/// back to "not spam" rather than acting on noise).
pub async fn score(
    db: &PgPool,
    user_id: Uuid,
    tokens: &HashSet<String>,
) -> anyhow::Result<Option<f64>> {
    let stats: Option<(i32, i32)> = sqlx::query_as(
        "SELECT spam_messages, ham_messages FROM mail.spam_stats WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    let (nspam, nham) = match stats {
        Some((s, h)) => (s as f64, h as f64),
        None => return Ok(None),
    };
    if nspam < 1.0 || nham < 1.0 || (nspam + nham) < MIN_TRAIN_MESSAGES as f64 {
        return Ok(None);
    }
    if tokens.is_empty() {
        return Ok(None);
    }

    let toks: Vec<String> = tokens.iter().cloned().collect();
    let rows: Vec<(String, i32, i32)> = sqlx::query_as(
        "SELECT token, spam_count, ham_count FROM mail.spam_tokens WHERE user_id = $1 AND token = ANY($2)",
    )
    .bind(user_id)
    .bind(&toks)
    .fetch_all(db)
    .await?;

    let mut probs: Vec<f64> = Vec::with_capacity(rows.len());
    for (_tok, sc, hc) in &rows {
        if let Some(p) = token_spamminess(*sc, *hc, nspam, nham) {
            probs.push(p);
        }
    }

    // Tokens present in the message but never seen → Graham's neutral 0.4.
    let seen: HashSet<&String> = rows.iter().map(|(t, _, _)| t).collect();
    let unknown = tokens.iter().filter(|t| !seen.contains(*t)).count();
    probs.extend(std::iter::repeat_n(UNKNOWN_PROB, unknown.min(TOP_N)));

    Ok(combine_probs(probs))
}

/// Spam probability of a single token, or `None` if it is too rare to trust.
/// Ham counts are weighted (Graham) and ratios are clamped to [0.01, 0.99].
fn token_spamminess(spam_count: i32, ham_count: i32, nspam: f64, nham: f64) -> Option<f64> {
    let b = spam_count as f64;
    let g = HAM_WEIGHT * (ham_count as f64);
    if b + g < MIN_TOKEN_WEIGHT {
        return None;
    }
    let pb = (b / nspam).min(1.0);
    let pg = (g / nham).min(1.0);
    Some((pb / (pb + pg)).clamp(0.01, 0.99))
}

/// Combine per-token probabilities into a single message score, keeping only
/// the `TOP_N` most informative tokens (farthest from 0.5). Returns `None` when
/// there is nothing to combine. P = ∏p / (∏p + ∏(1-p)).
fn combine_probs(mut probs: Vec<f64>) -> Option<f64> {
    if probs.is_empty() {
        return None;
    }
    probs.sort_by(|a, b| {
        (b - 0.5)
            .abs()
            .partial_cmp(&(a - 0.5).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    probs.truncate(TOP_N);

    let prod_p: f64 = probs.iter().product();
    let prod_np: f64 = probs.iter().map(|p| 1.0 - p).product();
    let denom = prod_p + prod_np;
    if denom == 0.0 {
        return None;
    }
    Some(prod_p / denom)
}

/// Settings + score for an incoming inbox message. Returns the computed
/// probability (if any) and whether it should be auto-moved to spam.
pub struct Verdict {
    pub score: Option<f64>,
    pub move_to_spam: bool,
}

/// Score an incoming message and decide whether to move it to spam, honouring
/// the user's `auto_classify` flag and `threshold`.
pub async fn classify_incoming(
    db: &PgPool,
    user_id: Uuid,
    subject: &str,
    body: Option<&str>,
    from_email: &str,
) -> anyhow::Result<Verdict> {
    let settings: Option<(bool, f32)> = sqlx::query_as(
        "SELECT auto_classify, threshold FROM mail.spam_stats WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    let (auto, threshold) = match settings {
        Some((a, t)) => (a, t as f64),
        None => return Ok(Verdict { score: None, move_to_spam: false }),
    };

    let tokens = tokenize(subject, body, from_email);
    let score = score(db, user_id, &tokens).await?;
    let move_to_spam = auto && matches!(score, Some(p) if p >= threshold);
    Ok(Verdict { score, move_to_spam })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_and_keeps_sender() {
        let toks = tokenize("Cheap VIAGRA now", Some("Buy at 50% off!!!"), "promo@spam.example");
        assert!(toks.contains("cheap"));
        assert!(toks.contains("viagra"));
        assert!(toks.contains("from:promo@spam.example"));
        assert!(toks.contains("fromdom:spam.example"));
        // Pure numbers and 1-char fragments are dropped.
        assert!(!toks.contains("50"));
        assert!(!toks.contains(""));
    }

    #[test]
    fn tokenize_dedupes_within_message() {
        // "spam" repeated counts once (binary presence per message).
        let toks = tokenize("spam spam spam", None, "");
        assert_eq!(toks.iter().filter(|t| *t == "spam").count(), 1);
    }

    #[test]
    fn token_spamminess_rare_token_ignored() {
        // b + 2*g = 1 < MIN_TOKEN_WEIGHT (5) → not trusted.
        assert_eq!(token_spamminess(1, 0, 100.0, 100.0), None);
    }

    #[test]
    fn token_spamminess_spammy_vs_hammy() {
        // Appears in 20 spam, never ham → near the 0.99 ceiling.
        let spammy = token_spamminess(20, 0, 100.0, 100.0).unwrap();
        assert!(spammy > 0.9, "spammy={spammy}");
        // Appears in 20 ham, never spam → near the 0.01 floor.
        let hammy = token_spamminess(0, 20, 100.0, 100.0).unwrap();
        assert!(hammy < 0.1, "hammy={hammy}");
    }

    #[test]
    fn combine_pushes_toward_dominant_class() {
        // Several strongly spammy tokens → score close to 1.
        let spammy = combine_probs(vec![0.99, 0.98, 0.97, 0.95]).unwrap();
        assert!(spammy > 0.99, "spammy={spammy}");
        // Several strongly hammy tokens → score close to 0.
        let hammy = combine_probs(vec![0.01, 0.02, 0.03, 0.05]).unwrap();
        assert!(hammy < 0.01, "hammy={hammy}");
    }

    #[test]
    fn combine_neutral_tokens_stay_mid() {
        let p = combine_probs(vec![0.5, 0.5, 0.5]).unwrap();
        assert!((p - 0.5).abs() < 1e-9, "p={p}");
    }

    #[test]
    fn combine_empty_is_none() {
        assert_eq!(combine_probs(vec![]), None);
    }
}
