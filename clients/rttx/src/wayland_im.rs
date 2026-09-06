//! Startup workaround for a use-after-free in GTK's Wayland input-method
//! backend (`GtkIMContextWayland`, GTK 4.20.4 and current `main`).
//!
//! # The GTK bug
//!
//! `GtkIMContextWayland` keeps one per-display "global" struct holding an
//! **unowned** `current` pointer to whichever IM context last received
//! `focus_in`.  The global is created lazily on the first `focus_in`, which
//! also sends a `wl_registry.get_registry` request; the compositor's reply
//! (announcing `zwp_text_input_manager_v3`) is only dispatched on a later
//! main-loop iteration.  In between, `global->text_input` is `NULL`.
//!
//! * `focus_in` sets `global->current = ctx` *before* checking `text_input`.
//! * `focus_out` (also reached from `set_client_widget(NULL)` and `finalize`)
//!   returns early while `text_input == NULL` and never clears `current`.
//!
//! So an IM context that is focused and then destroyed inside that one
//! round-trip window leaves `global->current` dangling.  The first
//! `zwp_text_input_v3.enter`/`done` event then calls
//! `notify_im_change(freed ctx)` → `gtk_im_context_wayland_get_global` →
//! `gtk_widget_get_display(garbage)` → SIGSEGV.
//!
//! rttx hits this at startup: the first endpoint reconciliation runs
//! `rebuild_session_content`, which removes the workspace page from the
//! `GtkStack` and rebuilds it.  That unrealizes every VTE, and VTE tears
//! down its `GtkIMMulticontext` on unrealize.  If the compositor had already
//! handed keyboard focus to a VTE (typical right after login or a fresh
//! launch, when the compositor is busy and replies late), the race is lost.
//!
//! # The workaround
//!
//! Close the window before any real widget can be focused: create a
//! throwaway `IMMulticontext`, focus it (creates the global and sends the
//! registry request), round-trip with `gdk_display_sync`, and unfocus it —
//! which now clears `current` correctly because `text_input` is bound.
//! After that, `text_input` is never `NULL` again for the life of the
//! display, so the early-return in `focus_out` cannot be reached.
//!
//! The primer context and its anchor widget are kept alive for the rest of
//! the process, and the client widget is never unset (that would make
//! `GtkIMMulticontext` finalize its Wayland delegate).  This is the safety
//! net for a compositor that advertises text-input in GDK's registry
//! snapshot but never delivers it to GTK's second registry: `current`
//! would still point at the primer's delegate, and a live delegate with a
//! live widget is harmless to every text-input event handler.
//!
//! Tracked in rttx issue #1099 (supersedes #808).  Remove once the upstream
//! fix ships in the minimum supported GTK.

use gtk4::prelude::*;
use std::cell::RefCell;

thread_local! {
    /// `(primer, anchor)` — both must outlive every text-input event.
    static PRIMER: RefCell<Option<(gtk4::IMMulticontext, gtk4::Label)>> =
        const { RefCell::new(None) };
}

/// What [`prime_text_input`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimingOutcome {
    /// The Wayland IM global was created and its registry round-trip
    /// completed; `focus_out` now clears `current` reliably.
    Primed,
    /// A previous call on this thread already primed the display.
    AlreadyPrimed,
    /// Not a Wayland display; `GtkIMContextWayland` is never used.
    NotWayland,
    /// No default display (headless test runs).
    NoDisplay,
}

/// Whether a GDK display of this `GType` name routes IM through
/// `GtkIMContextWayland`.
///
/// Only `GdkWaylandDisplay` does.  X11 and Broadway select
/// `GtkIMContextSimple` (or `IBus`) and are not affected.
#[must_use]
pub fn backend_needs_priming(display_type_name: &str) -> bool {
    display_type_name == "GdkWaylandDisplay"
}

/// Run the priming sequence on the default display.
///
/// Call once from the application `startup` handler, before any window is
/// presented.  Safe to call again (returns [`PrimingOutcome::AlreadyPrimed`])
/// and safe on non-Wayland backends (returns [`PrimingOutcome::NotWayland`]).
pub fn prime_text_input() -> PrimingOutcome {
    let Some(display) = gtk4::gdk::Display::default() else {
        return PrimingOutcome::NoDisplay;
    };
    if !backend_needs_priming(display.type_().name()) {
        return PrimingOutcome::NotWayland;
    }
    if PRIMER.with(|slot| slot.borrow().is_some()) {
        return PrimingOutcome::AlreadyPrimed;
    }

    // Any widget works as the client: `gtk_widget_get_display` falls back to
    // the default display for an unrooted widget, and that is all
    // `focus_in` needs to find (or create) the per-display global.
    let anchor = gtk4::Label::new(None);
    let primer = gtk4::IMMulticontext::new();
    primer.set_client_widget(Some(&anchor));

    // Creates GtkIMContextWaylandGlobal, sends wl_registry.get_registry and
    // records `global->current = primer` (text_input is still NULL here).
    primer.focus_in();

    // wl_display_roundtrip: the registry reply is dispatched synchronously,
    // binding zwp_text_input_manager_v3 and creating global->text_input.
    display.sync();

    // With text_input bound, focus_out takes the full path and clears
    // `global->current`.  Do NOT unset the client widget: GtkIMMulticontext
    // would finalize the Wayland delegate, and the delegate must stay alive
    // in case `current` still names it (see module docs).
    primer.focus_out();

    tracing::debug!("Primed GTK Wayland text-input global before first window");
    PRIMER.with(|slot| *slot.borrow_mut() = Some((primer, anchor)));
    PrimingOutcome::Primed
}

/// Whether this thread has already primed the display.
#[must_use]
pub fn is_primed() -> bool {
    PRIMER.with(|slot| slot.borrow().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_wayland_displays_need_priming() {
        assert!(backend_needs_priming("GdkWaylandDisplay"));
        assert!(!backend_needs_priming("GdkX11Display"));
        assert!(!backend_needs_priming("GdkBroadwayDisplay"));
        assert!(!backend_needs_priming("GdkMacosDisplay"));
        assert!(!backend_needs_priming(""));
    }
}
