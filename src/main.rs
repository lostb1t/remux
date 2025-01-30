use dioxus::prelude::*;
use dioxus_logger::tracing::{info, Level};
use jellyfin_api;
use jellyfin_api::Client;

use components::Navbar;
use views::{Home, Settings};
// use crate::hooks::*;
use remux_web::hooks::*;

mod clients;
mod components;
mod views;

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

// #[derive(Clone, Copy)]
// struct Session {
//     client: jellyfin_api::Client,
//     user: Option<jellyfin_api::types::User>
// }

// impl Session {
//   fn from_storage() -> Self {
//     Self {
//       client: jellyfin_api::Client::new("https://jellyfin.sjoerdarendsen.dev"),
//       user: None
//     }
//   }
// }

// #[derive(Clone, Copy)]
// struct Settings {
// }



#[component]
fn JellyFinProvider(children: Element) -> Element {
    let client = use_client();
    let mut app = use_app();
    //let user = use_user();

    use_future(move || {
        to_owned![client];
        async move {
            let result = client
                .authenticate_user_by_name()
                .body(
                    jellyfin_api::types::AuthenticateUserByName::builder()
                        .pw("myfmor-6viXpo-vidhyr".to_string())
                        .username("sarendsen".to_string()),
                )
                .send()
                .await;
            // info!("{:?}", &result);
            app.user.set(Some(result.unwrap().into_inner()));
            // dbg!(&result);
            // info!("{:?}", &result);
            
            // jellyfin_api::builder::AuthenticateUserByName("sjoerd", "password").await.unwrap()
        }
    });

    rsx! {
        {children}
    }
}



#[component]
fn App() -> Element {
    info!("App starting");
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::AUTHORIZATION, reqwest::header::HeaderValue::from_static("MediaBrowser Client=\"Android TV\", Device=\"Nvidia Shield\", DeviceId=\"ZQ9YQHHrUzk24vV\", Version=\"10.10.5\""));
    let rclient = reqwest::ClientBuilder::new()
        .default_headers(headers) // Add this line to the generated code
        .build()
        .unwrap();
    //let client = jellyfin_api::Client::new("https://jellyfin.sjoerdarendsen.dev");
    let client =
        jellyfin_api::Client::new_with_client("https://jellyfin.sjoerdarendsen.dev", rclient);
    use_context_provider(|| client);
    use_context_provider(|| AppState::default());
    //let user = use_context_provider(|| Signal::new(None));

    // client.authenticate_user_by_name()
    //     .body(body)
    //     .send()
    //     .await;
    // use_context_provider(|| client);

    // use_context_provider(move || async move {
    //     let client = jellyfin_api::Client::new("https://jellyfin.sjoerdarendsen.dev");
    //     client
    //         .authenticate_user_by_name()
    //         .body(
    //             jellyfin_api::types::AuthenticateUserByName::builder()
    //                 .pw("sjoerd".to_string())
    //                 .username("password".to_string()),
    //         )
    //         .send()
    //         .await;
    //     client
    //     // jellyfin_api::builder::AuthenticateUserByName("sjoerd", "password").await.unwrap()
    // });

    // let auth = use_future(move || async move {
    //     jellyfin_api::AuthenticateUserByName("sjoerd", "password").await.unwrap();
    // });

    // use_context_provider(|| Session.from_storage());

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
    //use_context_provider(|| client);

    rsx! {
        // Global app resources
        document::Link { rel: "icon", href: FAVICON }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: DAISY_CSS }

        JellyFinProvider {
            Router::<Route> {}
        }
    }
}
