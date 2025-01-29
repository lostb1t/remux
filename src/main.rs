use dioxus::prelude::*;
use dioxus_logger::tracing::{Level, info};
use jellyfin_api;
use jellyfin_api::{Client, AuthenticateUserByName};

use components::Navbar;
use views::{Home, Settings};

mod components;
mod views;
mod clients;

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Navbar)]
    #[route("/")]
    Home {},
    #[route("/settings")]
    Settings {},
    // #[route("/blog/:id")]
    // Blog { id: i32 },
}

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const DAISY_CSS: &str = "https://cdn.jsdelivr.net/npm/daisyui@4.12.23/dist/full.min.css";

fn main() {
    dioxus_logger::init(Level::INFO).expect("logger failed to init");
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let client = jellyfin_api::Client::new("https://jellyfin.sjoerdarendsen.dev");
    use_context_provider(|| client);

    // let auth = use_future(move || async move {
    //     jellyfin_api::AuthenticateUserByName("sjoerd", "password").await.unwrap();
    // });

    // let mut favorites = use_resource(crate::backend::list_dogs).suspend()?;

    // jellyfin_api::types::AuthenticateUserByName("sjoerd", "password").unwrap();

    // let test = use_context_provider(move || async move {
    //     jellyfin_api::builder::AuthenticateUserByName("sjoerd", "password").await.unwrap()
    // });

    // let manager = match auth.value() {
    //     Some(manager) => manager.to_owned(),
    //     None => panic!("yo"),
    // };

    // dbg!(manager);
    // let result = jellyfin_api::AuthenticateUserByName("sjoerd", "password").await.unwrap();
    // client.AuthenticateUserByName("sjoerd", "password").await.unwrap();
    // jellyfin_api::AuthenticateUserByName("sjoerd", "password").await.unwrap();
    // match client.authenticate("your-username", "your-password") {
    //     Ok(_) => {
    //         println!("Authentication successful!");
    //         // Proceed with using the authenticated client
    //     }
    //     Err(e) => {
    //         eprintln!("Authentication failed: {}", e);
    //         // Handle authentication failure
    //     }
    // }
    use_context_provider(|| client);

    rsx! {
        // Global app resources
        document::Link { rel: "icon", href: FAVICON }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: DAISY_CSS }

        Router::<Route> {}
    }
}
