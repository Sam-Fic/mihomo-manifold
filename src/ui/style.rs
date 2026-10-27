//! Shared CSS.
//!
//! Only what libadwaita does not already provide lives here. `.card` is close to
//! what the log view wants, but not close enough: a `GtkTextView` paints its own
//! background (see `textview > border` in Adwaita's own stylesheet), which covers
//! the rounded corners of the frame around it. The fix is the one the sibling
//! doubao-input app uses for its diagnostic report — give the frame an explicit
//! background and border, then make the text view transparent so the frame shows
//! through.
//!
//! Colours are named colours from the Adwaita stylesheet, not literals, so the
//! same rule works in light, dark and high-contrast themes: `@view_bg_color` is
//! the "content" background (slightly darker than a window in dark mode) and
//! `@borders` follows `prefers-contrast`.

use std::sync::OnceLock;

/// Frame around a read-only text area.
pub const TEXT_FRAME: &str = "text-frame";

const CSS: &str = "\
.text-frame {
  border-radius: 12px;
  background-color: @view_bg_color;
  border: 1px solid @borders;
  padding: 4px;
}
.text-frame textview,
.text-frame textview > text,
.text-frame textview > border {
  background: transparent;
}
";

/// Install the stylesheet once per process, at application priority so it wins
/// over the theme but still loses to anything a user has put in `gtk.css`.
///
/// Safe to call more than once: GTK would otherwise pile up providers.
pub fn install() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        let provider = gtk::CssProvider::new();
        // `load_from_string` is the 4.12+ spelling; `load_from_data` is deprecated.
        // The crate is built with the v4_14 feature, so this always exists.
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}
