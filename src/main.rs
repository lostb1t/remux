use dioxus::prelude::*;
use dioxus_motion::prelude::*;
use dioxus_motion::transitions::page_transitions::AnimatedOutlet;
use dioxus_logger::tracing::{info, Level};


use components::Navbar;
//use remux_web::server::{Jellyfin, Server, Servers};
use views::{Home, settings::Settings,};
// use crate::hooks::*;
//use remux_web::hooks::*;

mod clients;
mod components;
mod views;
mod addons;

#[derive(Debug, Clone, Routable, PartialEq, MotionTransitions)]
#[rustfmt::skip]
enum Route {
    #[layout(Navbar)]
    #[route("/")]
    Home {},
    #[route("/settings")]
    Settings {},
  //  #[route("/settings/server/create")]
  //  SettingsServersAdd {},
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

// #[component]
// fn ServersProvider(children: Element) -> Element {
//     let client = use_client();
//     let mut app = use_app();
//     //let user = use_user();

//     use_future(move || {
//         to_owned![client];
//         async move {
//             let result = client
//                 .authenticate_user_by_name()
//                 .body(
//                     jellyfin_api::types::AuthenticateUserByName::builder()
//                         .pw("myfmor-6viXpo-vidhyr".to_string())
//                         .username("sarendsen".to_string()),
//                 )
//                 .send()
//                 .await;
//             // info!("{:?}", &result);
//             app.user.set(Some(result.unwrap().into_inner()));
//             // dbg!(&result);
//             // info!("{:?}", &result);

//             // jellyfin_api::builder::AuthenticateUserByName("sjoerd", "password").await.unwrap()
//         }
//     });

//     rsx! {
//         {children}
//     }
// }

#[component]
fn ServerProvider(children: Element) -> Element {
   // let mut servers = use_servers();
    // let mut app = use_app();
    //let user = use_user();

    use_future(move || {
        // to_owned![servers];
        async move {
            info!("CServer provider");
            
            // // use_context_provider(move || servers.clone());
            //servers.set(s);
            // dbg!(&result);
            // info!("{:?}", &result);

            // jellyfin_api::builder::AuthenticateUserByName("sjoerd", "password").await.unwrap()
        }
    });

    rsx! {
        {children}
    }
}


// #[derive(Clone, Copy)]
// struct App {
//     servers: Signal<Servers>,
// }

#[component]
fn App() -> Element {
    info!("App starting");

    // let mut jf = Jellyfin {
    //   host: "https://jellyfin.sjoerdarendsen.dev".to_string(),
    //   username: "sarendsen".to_string(),
    //   password: "myfmor-6viXpo-vidhyr".to_string(),
    //   auth_token: None,
    //   client: None
    // };

    // //let servers: Servers = vec![Box::new(jf)];

    // let servers = async {
    //     jf.connect();
    //     let servers: Servers = vec![Box::new(jf)];
    //     Signal::new(servers)
    // };

    // let servers: Servers = vec![];
   // use_context_provider(|| Signal::new(Servers::default()));
   // use_context_provider(|| AppState::default());
    //let user = use_context_provider(|| Signal::new(None));

    // client.authenticate_user_by_name()
    //     .body(body)
    //     .send()
    //     .await;
    // use_context_provider(|| client);

    // let test = use_context_provider(move || async move {
    //     Signal::new(Servers::default())
    //     // let mut jf = Jellyfin {
    //     //     host: "https://jellyfin.sjoerdarendsen.dev".to_string(),
    //     //     username: "sarendsen".to_string(),
    //     //     password: "myfmor-6viXpo-vidhyr".to_string(),
    //     //     auth_token: None,
    //     //     client: None,
    //     // };
    //     // jf.connect().await;
    //     // Signal::new(vec![jf])
    // });
    // let servers = use_future(move || async move {
    //     let mut jf = Jellyfin {
    //         host: "https://jellyfin.sjoerdarendsen.dev".to_string(),
    //         username: "sarendsen".to_string(),
    //         password: "myfmor-6viXpo-vidhyr".to_string(),
    //         auth_token: None,
    //         client: None,
    //     };
    //     jf.connect().await;
    //     vec![jf]
    // });

    // use_context_provider(move || async move {
    //     servers.clone()
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

        ServerProvider {
            Router::<Route> {}
        }
    }
}
