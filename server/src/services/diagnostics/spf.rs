//! SPF: is a record published, and does it stay under the ten-lookup ceiling?
//!
//! ── Why the count matters more than the record ──────────────────────────────
//! RFC 7208 §4.6.4 caps at TEN the number of DNS-querying mechanisms a verifier
//! must follow. Past that, the verdict is `permerror`, which DMARC treats as a
//! failure — the mail is rejected exactly as if SPF said so. The ceiling is
//! crossed by *nesting*: three `include:` of providers who each `include:` two
//! more, and a record that reads as fine is already dead. Nothing in an SPF
//! record shows this, which is why every serious tool counts it, recursively.
//!
//! ── What this deliberately does not do ──────────────────────────────────────
//! It does not say a record is correct. Whether `include:` a given provider, or
//! end in `~all` rather than `-all`, is the operator's call and depends on who
//! else sends for the domain. The record is shown, the count is given, and the
//! judgement is left where it belongs. Only two things are asserted: an absent
//! record (mail rejected outright) and a count over ten (permerror) are wrong.

use hickory_resolver::TokioResolver;

use super::dns::{txt, Answer};
use super::{Check, Verdict};

/// RFC 7208 §4.6.4.
const MAX_LOOKUPS: u32 = 10;

/// Stop following after this many, whatever the record says: an `include:` loop
/// (or a chain built to be one) must not turn a diagnostic into a DNS flood.
/// Above the ceiling the exact number no longer matters — it is already wrong.
const HARD_STOP: u32 = 20;

/// How deep the `include:`/`redirect=` chain is followed.
const MAX_DEPTH: u32 = 10;

/// Result of walking the record.
struct Count {
    lookups: u32,
    /// A nested record could not be fetched — the count is a lower bound.
    partial: bool,
    /// Walking stopped at `HARD_STOP`.
    capped:  bool,
}

pub async fn check(resolver: &TokioResolver, domain: &str) -> Check {
    let expected = format!("{domain}.  IN TXT  \"v=spf1 mx ~all\"");

    let records = match txt(resolver, domain).await {
        Answer::Records(r) => r,
        Answer::Unavailable => {
            return Check::new("spf", domain, Verdict::Unknown,
                "Impossible d'interroger le DNS pour cet enregistrement.")
                .expected(expected)
        }
        Answer::Absent => {
            return Check::new("spf", domain, Verdict::Fail,
                "Aucun enregistrement TXT : ce domaine n'a pas de SPF, et son courrier est refusé \
                 ou classé indésirable par les grands fournisseurs.")
                .expected(expected)
        }
    };

    let spf: Vec<String> = records.into_iter().filter(|r| is_spf(r)).collect();

    if spf.is_empty() {
        return Check::new("spf", domain, Verdict::Fail,
            "Aucun enregistrement SPF (aucun TXT ne commence par « v=spf1 »).")
            .expected(expected)
    }
    if spf.len() > 1 {
        // RFC 7208 §3.2: more than one record is a permerror, i.e. no SPF at all.
        return Check::new("spf", domain, Verdict::Fail,
            "Plusieurs enregistrements SPF publiés. La spécification impose un seul : \
             les vérificateurs répondent « permerror » et le SPF ne protège plus rien.")
            .expected(expected).found(spf)
    }

    let record = spf[0].clone();
    let count = count_lookups(resolver, &record, 0, &mut 0).await;

    let detail = format!(
        "{} résolution{} DNS sur les {MAX_LOOKUPS} autorisées",
        count.lookups,
        if count.lookups > 1 { "s" } else { "" },
    );

    if count.capped || count.lookups > MAX_LOOKUPS {
        Check::new("spf", domain, Verdict::Fail,
            format!("Le plafond de {MAX_LOOKUPS} résolutions DNS est dépassé ({detail}). \
                     Les vérificateurs répondent « permerror » : c'est un échec SPF, \
                     puis un échec DMARC. Réduisez les « include: »."))
            .expected(expected).found(vec![record])
    } else if count.partial {
        Check::new("spf", domain, Verdict::Unknown,
            format!("Enregistrement publié, mais le décompte est incomplet : un « include: » \
                     n'a pas pu être résolu. Au moins {detail}."))
            .expected(expected).found(vec![record])
    } else {
        // Published and within the ceiling. Whether it lists the right senders
        // is not something this page can know — so: fact, not verdict.
        Check::new("spf", domain, Verdict::Info,
            format!("Enregistrement publié, {detail}. Vérifiez qu'il autorise bien tous vos \
                     expéditeurs et son mécanisme final (« ~all » ou « -all »)."))
            .expected(expected).found(vec![record])
    }
}

fn is_spf(record: &str) -> bool {
    let head = record.trim_start().to_ascii_lowercase();
    head == "v=spf1" || head.starts_with("v=spf1 ")
}

/// Counts the DNS-querying terms of `record`, following `include:` and
/// `redirect=`.
///
/// Boxed because the recursion is on an `async fn`, whose future would
/// otherwise have infinite size.
fn count_lookups<'a>(
    resolver: &'a TokioResolver,
    record: &'a str,
    depth: u32,
    spent: &'a mut u32,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Count> + Send + 'a>> {
    Box::pin(async move {
        let mut partial = false;
        let mut capped  = *spent >= HARD_STOP;
        let mut nested: Vec<String> = Vec::new();

        for term in record.split_whitespace().skip(1) {
            if *spent >= HARD_STOP {
                capped = true;
                break;
            }
            // A qualifier (+ - ~ ?) may prefix any mechanism.
            let term = term.trim_start_matches(['+', '-', '~', '?']);
            let (name, value) = match term.split_once([':', '=']) {
                Some((n, v)) => (n.to_ascii_lowercase(), Some(v)),
                None => (term.to_ascii_lowercase(), None),
            };

            // RFC 7208 §4.6.4: these — and only these — cost a lookup each.
            match name.as_str() {
                "a" | "mx" | "ptr" | "exists" => *spent += 1,
                "include" | "redirect" => {
                    *spent += 1;
                    if let Some(target) = value {
                        nested.push(target.to_string());
                    }
                }
                _ => {} // ip4, ip6, all, exp, v… cost nothing
            }
        }

        // Follow the chain only after counting this level, so the ceiling is
        // reached in the same order a verifier would reach it.
        if depth < MAX_DEPTH {
            for target in nested {
                if *spent >= HARD_STOP {
                    capped = true;
                    break;
                }
                match txt(resolver, &target).await {
                    // No SPF record at the target: a verifier gets permerror
                    // there, but the lookup was still spent — so it stays
                    // counted and nothing else is followed.
                    Answer::Records(records) => {
                        if let Some(inner) = records.into_iter().find(|r| is_spf(r)) {
                            let sub = count_lookups(resolver, &inner, depth + 1, spent).await;
                            partial |= sub.partial;
                            capped  |= sub.capped;
                        }
                    }
                    Answer::Absent => {}
                    Answer::Unavailable => partial = true,
                }
            }
        } else {
            capped = true;
        }

        Count { lookups: *spent, partial, capped }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts the terms of one record without touching the network — the part
    /// that is easy to get wrong (qualifiers, `ip4:` costing nothing).
    fn local_terms(record: &str) -> u32 {
        let mut spent = 0u32;
        for term in record.split_whitespace().skip(1) {
            let term = term.trim_start_matches(['+', '-', '~', '?']);
            let name = match term.split_once([':', '=']) {
                Some((n, _)) => n.to_ascii_lowercase(),
                None => term.to_ascii_lowercase(),
            };
            if matches!(name.as_str(), "a" | "mx" | "ptr" | "exists" | "include" | "redirect") {
                spent += 1;
            }
        }
        spent
    }

    #[test]
    fn ip_mechanisms_and_all_cost_nothing() {
        assert_eq!(local_terms("v=spf1 ip4:198.51.100.0/24 ip6:2001:db8::/32 -all"), 0);
    }

    #[test]
    fn queried_mechanisms_each_cost_one_qualifier_included() {
        assert_eq!(local_terms("v=spf1 mx a:mail.example.com ?include:_spf.example.net ~all"), 3);
    }

    #[test]
    fn redirect_costs_one() {
        assert_eq!(local_terms("v=spf1 redirect=_spf.example.com"), 1);
    }

    #[test]
    fn only_a_v_spf1_record_is_an_spf_record() {
        assert!(is_spf(" v=spf1 -all"));
        assert!(is_spf("V=SPF1 mx"));
        assert!(!is_spf("v=spf10 mx"));
        assert!(!is_spf("site-verification=abc"));
    }
}
