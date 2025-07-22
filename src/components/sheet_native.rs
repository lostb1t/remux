use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, info};
use std::ops::Deref;
use std::rc::Rc;
use std::time::Duration;

/// Component that acts as an bottom sheet (mobile style) on small screens. And as a modal on larger
#[component]
pub fn Sheet(open: Signal<bool>, title: Option<String>, children: Element) -> Element {
    rsx! {
        div {
          ""
        }
    }
}
