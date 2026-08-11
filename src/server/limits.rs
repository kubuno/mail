//! Connection hardening shared by all three protocols: a per-IP simultaneous
//! connection cap and an exponential auth tarpit.
//!
//! These are the two highest-value defences a mail server exposed to the
//! internet needs and that a plain idle timeout does not provide (Dovecot's
//! anvil + auth-penalty). Both are in-memory and best-effort: they protect a
//! single process, which is what this module is.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// Wall clock, injected so the tarpit is testable without sleeping. Production
/// uses `SystemClock`; tests use a controllable one.
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

/// Seconds since an arbitrary fixed point (process start). Monotonic.
pub struct SystemClock {
    origin: std::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self { origin: std::time::Instant::now() }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

// ── Per-IP simultaneous connection cap ───────────────────────────────────────

/// Counts live connections per client IP and refuses new ones past a limit.
/// Prevents a single host from exhausting file descriptors / worker tasks.
///
/// The cap is adjustable in place, for the same reason the tarpit's switch is:
/// building a new limiter on every reconfiguration would start the counts back
/// at zero while the connections they were counting are still open, letting one
/// host briefly hold twice its allowance.
pub struct ConnectionLimiter {
    max_per_ip: AtomicU32,
    counts:     Arc<Mutex<HashMap<IpAddr, u32>>>,
}

impl ConnectionLimiter {
    pub fn new(max_per_ip: u32) -> Self {
        Self {
            max_per_ip: AtomicU32::new(max_per_ip.max(1)),
            counts:     Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Applies a new cap to the live limiter. Connections already accepted are
    /// never closed by a lowered cap — it applies to the next one, as Dovecot's
    /// anvil does.
    pub fn set_max_per_ip(&self, max_per_ip: u32) {
        self.max_per_ip.store(max_per_ip.max(1), Ordering::Relaxed);
    }

    /// Registers a new connection from `ip`. Returns a guard that keeps the slot
    /// until dropped, or `None` when the IP is already at its limit.
    pub fn acquire(&self, ip: IpAddr) -> Option<ConnGuard> {
        let max = self.max_per_ip.load(Ordering::Relaxed);
        let mut counts = self.counts.lock().ok()?;
        let n = counts.entry(ip).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(ConnGuard { ip, counts: self.counts.clone() })
    }
}

/// Releases one connection slot for its IP when dropped.
pub struct ConnGuard {
    ip:     IpAddr,
    counts: Arc<Mutex<HashMap<IpAddr, u32>>>,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        if let Ok(mut counts) = self.counts.lock() {
            if let Some(n) = counts.get_mut(&self.ip) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    counts.remove(&self.ip);
                }
            }
        }
    }
}

// ── Exponential auth tarpit ──────────────────────────────────────────────────

/// The delay grows 0 → 2 → 4 → 8 → 15s and stays at 15s. Slows brute force to a
/// crawl without ever locking a legitimate user out.
const TARPIT_STEPS: [u64; 5] = [0, 2, 4, 8, 15];
/// A penalty is forgotten after this long with no new failure.
const PENALTY_TTL: Duration = Duration::from_secs(3600);

struct Penalty {
    strikes:   u32,
    last_seen: Duration,
}

/// Per-IP failed-auth penalty. After each failure the caller waits
/// `delay_for(ip)` before sending the failure reply; a success clears it.
///
/// The whole thing can be switched off from the console. It stays a single
/// long-lived instance either way: the toggle is an atomic flag, not a new
/// object, so turning the penalty off and on again does not hand every prober
/// on the internet a clean slate.
pub struct Tarpit {
    clock:   Box<dyn Clock>,
    enabled: AtomicBool,
    ips:     Mutex<HashMap<IpAddr, Penalty>>,
}

impl Tarpit {
    /// Enabled by default, like Dovecot: a fresh instance protects itself, and
    /// it takes an explicit setting to stop it.
    pub fn new(clock: Box<dyn Clock>) -> Self {
        Self { clock, enabled: AtomicBool::new(true), ips: Mutex::new(HashMap::new()) }
    }

    /// Applies the administrator's current choice. Called on every
    /// reconfiguration; `&self` because the instance is shared by every live
    /// session and must not be replaced.
    ///
    /// `Relaxed` is enough: the flag guards nothing but itself, and a session
    /// that reads the previous value for a few microseconds around the change
    /// is of no consequence.
    pub fn set_enabled(&self, enabled: bool) {
        if self.enabled.swap(enabled, Ordering::Relaxed) != enabled {
            tracing::info!(actif = enabled, "Pénalité d'authentification par IP");
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// How long the failure reply for `ip` should be held back, given its
    /// current strike count. Does not itself change the count.
    ///
    /// This is the single choke point every protocol goes through, so returning
    /// zero here is what "penalty disabled" means: no sleep anywhere, on any
    /// port, without each handler having to know about the setting.
    pub fn delay_for(&self, ip: IpAddr) -> Duration {
        if !self.is_enabled() {
            return Duration::ZERO;
        }
        let now = self.clock.now();
        let ips = match self.ips.lock() {
            Ok(g) => g,
            Err(_) => return Duration::ZERO,
        };
        match ips.get(&ip) {
            Some(p) if now.saturating_sub(p.last_seen) < PENALTY_TTL => {
                let idx = (p.strikes as usize).min(TARPIT_STEPS.len() - 1);
                Duration::from_secs(TARPIT_STEPS[idx])
            }
            _ => Duration::ZERO,
        }
    }

    /// Records a failed authentication for `ip`, bumping its strike count. Also
    /// evicts expired entries opportunistically so the map stays bounded.
    ///
    /// Kept counting even while the penalty is off. Dovecot skips the update in
    /// that state, but there it means "anvil is unreachable", i.e. there is
    /// nowhere to write; here it means an administrator flipped a switch, and an
    /// operator who turns the penalty back on mid-attack wants it to bite
    /// immediately rather than to start the ladder again from zero.
    pub fn record_failure(&self, ip: IpAddr) {
        let now = self.clock.now();
        if let Ok(mut ips) = self.ips.lock() {
            ips.retain(|_, p| now.saturating_sub(p.last_seen) < PENALTY_TTL);
            let entry = ips.entry(ip).or_insert(Penalty { strikes: 0, last_seen: now });
            entry.strikes = entry.strikes.saturating_add(1);
            entry.last_seen = now;
        }
    }

    /// A success wipes the penalty — a legitimate user who finally logs in is no
    /// longer slowed.
    pub fn record_success(&self, ip: IpAddr) {
        if let Ok(mut ips) = self.ips.lock() {
            ips.remove(&ip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    struct TestClock(Arc<AtomicU64>);
    impl Clock for TestClock {
        fn now(&self) -> Duration {
            Duration::from_secs(self.0.load(Ordering::SeqCst))
        }
    }

    #[test]
    fn connection_cap_refuses_past_the_limit() {
        let lim = ConnectionLimiter::new(2);
        let a = lim.acquire(ip(1));
        let b = lim.acquire(ip(1));
        assert!(a.is_some() && b.is_some());
        assert!(lim.acquire(ip(1)).is_none(), "3e connexion refusée");
        // A different IP is unaffected.
        assert!(lim.acquire(ip(2)).is_some());
        // Dropping a guard frees a slot.
        drop(a);
        assert!(lim.acquire(ip(1)).is_some());
    }

    /// Raising the cap must not forget who is already connected: the two live
    /// connections still count against the new limit of three.
    #[test]
    fn changing_the_cap_keeps_the_live_counts() {
        let lim = ConnectionLimiter::new(2);
        let _a = lim.acquire(ip(1)).expect("1re");
        let _b = lim.acquire(ip(1)).expect("2e");
        assert!(lim.acquire(ip(1)).is_none());

        lim.set_max_per_ip(3);
        let _c = lim.acquire(ip(1)).expect("3e, autorisée par le nouveau plafond");
        assert!(lim.acquire(ip(1)).is_none(), "et pas une de plus");

        // Lowering it below the live count refuses new connections without
        // closing the ones already accepted.
        lim.set_max_per_ip(1);
        assert!(lim.acquire(ip(1)).is_none());
    }

    /// A cap of zero would refuse every connection; the floor of one keeps the
    /// service reachable whatever an administrator types.
    #[test]
    fn a_zero_cap_is_clamped_to_one() {
        let lim = ConnectionLimiter::new(0);
        assert!(lim.acquire(ip(1)).is_some());
        let lim = ConnectionLimiter::new(5);
        lim.set_max_per_ip(0);
        assert!(lim.acquire(ip(2)).is_some());
    }

    fn tarpit_at(secs: Arc<AtomicU64>) -> Tarpit {
        Tarpit::new(Box::new(TestClock(secs)))
    }

    #[test]
    fn tarpit_grows_then_caps_and_resets_on_success() {
        let t = tarpit_at(Arc::new(AtomicU64::new(0)));
        assert_eq!(t.delay_for(ip(1)), Duration::ZERO);
        t.record_failure(ip(1));
        assert_eq!(t.delay_for(ip(1)), Duration::from_secs(2));
        t.record_failure(ip(1));
        assert_eq!(t.delay_for(ip(1)), Duration::from_secs(4));
        for _ in 0..10 {
            t.record_failure(ip(1));
        }
        assert_eq!(t.delay_for(ip(1)), Duration::from_secs(15), "plafonné à 15s");
        t.record_success(ip(1));
        assert_eq!(t.delay_for(ip(1)), Duration::ZERO, "succès efface la pénalité");
    }

    #[test]
    fn penalty_expires_after_ttl() {
        let clock = Arc::new(AtomicU64::new(0));
        let t = tarpit_at(clock.clone());
        t.record_failure(ip(1));
        assert_eq!(t.delay_for(ip(1)), Duration::from_secs(2));
        clock.store(3601, Ordering::SeqCst); // > 1h later
        assert_eq!(t.delay_for(ip(1)), Duration::ZERO, "pénalité oubliée après 1h");
    }

    /// Off means off: whatever the strike count, nothing is delayed.
    #[test]
    fn a_disabled_tarpit_never_delays() {
        let t = tarpit_at(Arc::new(AtomicU64::new(0)));
        t.set_enabled(false);
        for _ in 0..5 {
            t.record_failure(ip(1));
        }
        assert_eq!(t.delay_for(ip(1)), Duration::ZERO);
        assert!(!t.is_enabled());
    }

    /// The administrator's toggle is applied to the live instance, and the
    /// strikes already collected are still there when it is turned back on —
    /// switching it off and on again must not reward an attacker.
    #[test]
    fn toggling_keeps_the_strikes_already_collected() {
        let t = tarpit_at(Arc::new(AtomicU64::new(0)));
        t.record_failure(ip(1));
        t.record_failure(ip(1));
        assert_eq!(t.delay_for(ip(1)), Duration::from_secs(4));

        t.set_enabled(false);
        assert_eq!(t.delay_for(ip(1)), Duration::ZERO, "désactivée = aucun délai");
        t.record_failure(ip(1)); // counted, but not acted on

        t.set_enabled(true);
        assert_eq!(t.delay_for(ip(1)), Duration::from_secs(8), "la progression a continué");
    }

    /// A success still clears the penalty while it is disabled, so re-enabling
    /// does not suddenly punish a client that has since authenticated.
    #[test]
    fn success_clears_the_penalty_even_while_disabled() {
        let t = tarpit_at(Arc::new(AtomicU64::new(0)));
        t.record_failure(ip(1));
        t.set_enabled(false);
        t.record_success(ip(1));
        t.set_enabled(true);
        assert_eq!(t.delay_for(ip(1)), Duration::ZERO);
    }
}
