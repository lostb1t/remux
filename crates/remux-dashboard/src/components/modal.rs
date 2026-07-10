use dioxus::prelude::*;

/// Width preset for a [`Modal`].
#[derive(Clone, Copy, PartialEq, Default)]
pub enum ModalSize {
    /// Default content width (440px).
    #[default]
    Md,
    /// Wider content width (600px) for dense or multi-field forms.
    Wide,
}

impl ModalSize {
    /// CSS class list for the inner dialog at this size.
    fn class(self) -> &'static str {
        match self {
            ModalSize::Md => "modal",
            ModalSize::Wide => "modal modal--wide",
        }
    }
}

/// A centered dialog rendered over a dimmed, full-screen backdrop.
///
/// UX guarantees (paired with the `.modal*` rules in `theme.css`):
/// * **Click-outside to close** — clicking the backdrop calls `on_close`.
/// * **Escape to close** — the backdrop is focused on mount and handles Esc.
/// * **Inside clicks are safe** — clicks within the dialog stop propagating, so
///   they never trigger the backdrop's close handler.
/// * **Never overflows the viewport** — the dialog is height-capped and its
///   `.modal-body` scrolls internally.
#[component]
pub fn Modal(
    /// Invoked when the user dismisses the modal (backdrop click or Escape).
    on_close: EventHandler,
    /// Width preset; defaults to [`ModalSize::Md`].
    #[props(default)]
    size: ModalSize,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "modal-backdrop",
            // Focusable (but out of the tab order) so it can receive the Escape
            // keydown; `autofocus` moves focus here when the modal mounts.
            tabindex: "-1",
            autofocus: true,
            onclick: move |_| on_close.call(()),
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    on_close.call(());
                }
            },
            div {
                class: size.class(),
                // Prevent inside clicks from bubbling to the backdrop's close.
                onclick: move |e| e.stop_propagation(),
                {children}
            }
        }
    }
}
