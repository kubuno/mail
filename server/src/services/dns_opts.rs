//! One place to build a DNS resolver, so every lookup this module makes —
//! delivery, message authentication, avatars, the administrator's diagnostic —
//! behaves the same way.
//!
//! ── Why this exists: truncation must fall back to TCP ────────────────────────
//! A DNS answer that does not fit in a UDP datagram comes back with the TC bit
//! set and must be re-asked over TCP. Mail depends on this: the TXT record set
//! at a domain's apex (which SPF reads) and a DKIM public key are routinely
//! larger than the EDNS payload a resolver advertises.
//!
//! hickory 0.26 does retry over TCP — but only while it is querying ONE server
//! at a time. Its pool queries `num_concurrent_reqs` servers in parallel
//! (default 2), and when a second server in the same batch also answers
//! truncated, the pool returns `Truncated` to the caller instead of retrying:
//! the truncation arm that re-queues the server is reached only for the first
//! one, and the second falls through to the "give up" arm. The visible effect
//! is a large TXT record that simply cannot be read — an SPF `temperror` on
//! perfectly ordinary mail, or a diagnostic that reports a published record as
//! missing.
//!
//! Querying one server at a time restores the fallback, and costs nothing that
//! matters here: the pool still moves on to the next server when one does not
//! answer, which is exactly how a stub resolver has always behaved. Revisit
//! when hickory handles truncation per server rather than per batch.

use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::net::NetError;
use hickory_resolver::TokioResolver;

/// Applies the module-wide resolver policy to `opts`.
pub fn harden(opts: &mut ResolverOpts) {
    opts.num_concurrent_reqs = 1;
}

/// The system resolver (`/etc/resolv.conf`), with that policy applied.
pub fn system_resolver() -> Result<TokioResolver, NetError> {
    let (config, opts) = system_conf()?;
    TokioResolver::builder_with_config(
        config,
        hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
    )
    .with_options(opts)
    .build()
}

/// The system configuration, with that policy applied — for the callers that
/// must hand the pair to another library rather than build the resolver here.
pub fn system_conf() -> Result<(ResolverConfig, ResolverOpts), NetError> {
    let (config, mut opts) = hickory_resolver::system_conf::read_system_conf()?;
    harden(&mut opts);
    Ok((config, opts))
}
