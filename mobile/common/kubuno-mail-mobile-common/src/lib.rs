//! Kubuno Mail for mobile - the app, portable (no OS, no window system).
//!
//! The app is a set of `.kbview` screens (`views/`, edited in Visual Studio like every Kubuno view) rendered by the
//! shared Kubuno mobile renderer (`kubuno-mobile-render`, core repository). The Android and iOS entry points
//! (`../android`, `../ios`) hand [`APP`] to the shared shell of their OS.
//!
//! Migration status: the screens are converted; the app's behaviour (its data and server calls) is still to be
//! migrated from the reference Kotlin app that lived in `mobile/` before this layout (kept in the git history).

use kubuno_mobile_render::App;

/// The `inbox.kbview` screen.
pub const INBOX: &str = include_str!("views/inbox.kbview");
/// The `compose.kbview` screen.
pub const COMPOSE: &str = include_str!("views/compose.kbview");
/// The `search.kbview` screen.
pub const SEARCH: &str = include_str!("views/search.kbview");
/// The `settings.kbview` screen.
pub const SETTINGS: &str = include_str!("views/settings.kbview");
/// The `thread.kbview` screen.
pub const THREAD: &str = include_str!("views/thread.kbview");

/// Every screen, in a tap-to-advance order (the home screen first).
pub const VIEWS: &[&str] = &[INBOX, COMPOSE, SEARCH, SETTINGS, THREAD];

/// Register what the app's screens need from the renderer (its host controls and images) - nothing yet. The shell
/// calls it once, on its UI thread, before the first frame (through [`APP`]).
pub fn setup() {}

/// The Kubuno Mail app, as the shells run it.
pub const APP: App = App { id: "mail", label: "Kubuno Mail", views: VIEWS, setup, on_event: None, view_index: None };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_renders() {
        setup();
        for src in VIEWS {
            assert!(kubuno_mobile_render::render_view(src, 1080, 2340, None, None).is_some());
        }
    }
}
