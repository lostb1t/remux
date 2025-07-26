#![cfg_attr(feature = "bundle", windows_subsystem = "windows")]
#![allow(warnings)]
use crate::server::{ConnectionStatus, Server};
use components::video::VideoPlayerState;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, error, info, trace, Level};
use dioxus_storage::set_dir;
use rand::rand_core::le;
use std::cell::OnceCell;
use std::sync::Arc;
use views::{
    media::MediaDetailViewTransition as MediaDetailView, settings::Settings,
    settings::SettingsCatalogView, AuthenticatedLayout, HomeTransitionView as Home, LoginView,
    MainLayout, SafeSpaceLayout, SearchView, UnauthenticatedLayout,
};

mod addons;
mod capabilities;
mod components;
mod error;
mod hooks;
mod js_bindings;
mod media;
mod sdks;
mod server;
mod settings;
mod utils;
mod views;

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(UnauthenticatedLayout)]
      #[route("/login")]
      LoginView {},

    #[layout(AuthenticatedLayout)]
        #[layout(MainLayout)]
            #[route("/")]
            Home {},
            #[route("/media/:media_type/:id")]
            MediaDetailView { media_type: media::MediaType, id: String },
         
            #[layout(SafeSpaceLayout)]
                #[route("/search/:query")]
                SearchView { query: String },
                #[route("/settings")]
                Settings {},
                #[route("/settings/catalog")]
                SettingsCatalogView {}
}

fn main() {
    set_dir!();

    dioxus_logger::init(Level::DEBUG).expect("logger failed to init");
    dioxus::launch(App);
}

#[derive(Props, Clone, PartialEq)]
pub struct LoadingProps {
    #[props(default = "".to_string())]
    class: String,
    pub children: Element,
    #[props(default = false)]
    transparant: bool,
}

#[component]
pub fn Loading(props: LoadingProps) -> Element {
    let bg = if !props.transparant {
        "bg-neutral-900/100"
    } else {
        ""
    };
    rsx! {
        div { id: "loading",
               //class: "fixed inset-0 z-40 flex items-center justify-center",
           class: "fixed inset-0 z-100 flex items-center justify-center {props.class} {bg}",

            div { role: "status", class: "flex flex-col items-center gap-2",

               div { class: "w-10 h-10 border-4 border-green-800/30 border-t-green-700 rounded-full animate-spin" }
               {props.children}
            }
        }
    }
}

#[component]
fn ServerProvider(children: Element) -> Element {
    // use_context_provider(|| Signal::new(None::<Arc<server::ServerInstance>>));
    let nav = use_navigator();
    let mut config_signal = hooks::use_server_config();
    let mut server_signal = hooks::use_server();
    let cfg = config_signal.peek().clone();
    let mut is_ready = use_signal(|| false);

    if !is_ready() {
        debug!("Not ready");
        // debug!("Server signal is None, checking config");
        if let Some(cfg) = &cfg {
            if let Ok(server) = server::ServerInstance::from_config(cfg.clone()) {
                debug!("Server initialized from config");
                server_signal.set(Some(Arc::new(server)));
                is_ready.set(true);
            } else {
                debug!("Server config failed");
                config_signal.set(None);
                is_ready.set(true);
            }
        } else {
            debug!("No config present");
            is_ready.set(true);
        }
    }
    // debug!("{:?} hello", config_signal.read());
    if config_signal.read().is_none() {
        debug!("Config is missing, routing to login");
        is_ready.set(true);
        nav.push(Route::LoginView {});
    }

    if server_signal.read().is_none() && config_signal.read().is_none() {
        debug!("Server and Config is missing, routing to login");
        is_ready.set(true);
        nav.push(Route::LoginView {});
    }

    if !is_ready() {
        return rsx! { Loading {} };
    }

    rsx! {
        {children}
    }
}

#[component]
fn ErrorHandler(children: Element) -> Element {
    rsx! {
        ErrorBoundary {
            handle_error: |e: ErrorContext| { rsx! {
                for e in e.errors() {
                    p { "{e}" }
                  //  {children.clone()}
                }
            } },
            {children}
        }
    }
}

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const MANIFEST: Asset = asset!("/assets/manifest.json");
const SW: Asset = asset!("/assets/sw.js");

pub static APP_HOST: GlobalSignal<utils::AppHost> = GlobalSignal::new(|| utils::AppHost::default());
pub static TMDB: GlobalSignal<sdks::tmdb::TmdbClient> = GlobalSignal::new(|| {
    sdks::tmdb::TmdbClient::new("https://api.themoviedb.org/3")
.unwrap()
.header("Authorization", "Bearer eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiIwZDczZTBjYjkxZjM5ZTY3MGIwZWZhNjkxM2FmYmQ1OCIsIm5iZiI6MTUzMjkzOTA3My41MzcsInN1YiI6IjViNWVjYjQxMGUwYTI2MmU5MDA0NjNjMCIsInNjb3BlcyI6WyJhcGlfcmVhZCJdLCJ2ZXJzaW9uIjoxfQ.vfOGe8_35CxhjjZXdnR2iAwdOMIY0VFYMBQrLWuRqn8")
});

#[component]
fn App() -> Element {
    info!("App starting");

    use_future(|| async {
        if let Some(caps) = capabilities::Capabilities::detect_browser_capabilities().await {
            use_context_provider(|| caps);
        }
    });

    use_context_provider(|| views::home::HomeFilter::default());
    use_context_provider(|| VideoPlayerState::default());
    use_context_provider(|| Signal::new(None::<Arc<server::ServerInstance>>));

    // let mut config_signal = hooks::use_server_config();
    // let cfg = config_signal.peek().clone();

    // use_context_provider(|| {
    //     debug!("YOHO");
    //     if let Some(cfg) = cfg {
    //         if let Ok(server) = server::ServerInstance::from_config(cfg) {
    //             debug!("server initialized");
    //             return Signal::new(Some(Arc::new(server)));
    //         }
    //         debug!("server config not ok");
    //         config_signal.set(None);
    //     }
    //     Signal::new(None::<Arc<server::ServerInstance>>)
    // });

    rsx! {
            // document::Link { rel: "icon", href: FAVICON }
            document::Link { rel: "manifest", href: MANIFEST }
            document::Script {
                {format!(r#"
  if (typeof navigator.serviceWorker !== 'undefined') {{
    navigator.serviceWorker.register('{SW}')
            }}
                  "#)}
            }
            document::Link { rel: "stylesheet", href: TAILWIND_CSS }
            document::Link { rel: "stylesheet", href: MAIN_CSS }
            document::Link { rel: "stylesheet", href: "https://cdnjs.cloudflare.com/ajax/libs/shaka-player/4.7.4/controls.min.css" }
            document::Script {
                src: "https://cdnjs.cloudflare.com/ajax/libs/shaka-player/4.7.7/shaka-player.compiled.min.js"
            }
            // document::Script { src: "https://cdnjs.cloudflare.com/ajax/libs/shaka-player/4.7.4/shaka-player.ui.min.js"}

    document::Script {
    {r#"

window.playShaka = async function(videoId, sourceUrl) {
    shaka.polyfill.installAll();

    const video = document.getElementById(videoId);
    const container = document.getElementById("Gidrocontsinet");

    if (!video || !container) {
        console.error("Video or container element not found");
        return;
    }

    if (!shaka.Player.isBrowserSupported()) {
        console.error("Shaka Player not supported");
        return;
    }

    const player = new shaka.Player();
    window._shaka_player = player;

    await player.attach(video, true);

    player.addEventListener('error', e => {
        console.error('Shaka error', e);
    });

    try {
        await player.load(sourceUrl);
        console.log("Shaka load successful");
        video.play().catch(e => console.warn("Autoplay blocked", e));
    } catch (e) {
        console.error('Shaka load failed', e);
    }
};
            "#}
            }

            document::Meta {
                name: "viewport",
                content: "viewport-fit=cover, user-scalable=no, width=device-width, initial-scale=1, maximum-scale=1",
            }
            document::Meta { name: "mobile-web-app-capable", content: "yes" }
            document::Meta { name: "apple-mobile-web-app-capable", content: "yes" }
            document::Meta {
                name: "apple-mobile-web-app-status-bar-style",
                content: "black-translucent",
            }

            div { class: "bg-neutral-900 min-h-screen",

            ErrorHandler {
                Router::<Route> {}
              }

        }


        }
}
