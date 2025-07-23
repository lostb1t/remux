#![cfg_attr(windows_subsystem = "windows")]
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
    settings::SettingsCatalogView, AuthenticatedLayout,
    HomeTransitionView as Home, LoginView, MainLayout, SafeSpaceLayout, SearchView,
    UnauthenticatedLayout,
};

mod addons;
mod capabilities;
mod components;
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
    let mut server = hooks::use_server();
    let nav = use_navigator();
    let mut config = hooks::use_server_config();
    let mut is_ready = use_signal(|| false);

    if server().is_none() && config().is_none() {
        debug!("Server and Config is missing, routing to login");
        nav.push(Route::LoginView {});
    };

    if config().is_none() && server().is_some() {
        debug!("server set but config missing. Should not happen....");
        config.set(Some(server().unwrap().into_config()));
    };

    use_future({
        let mut server_signal = server.clone();
        let mut config_signal = config.clone();
        let nav = nav.clone();

        move || async move {
            let reconnect_needed = match server_signal() {
                None => true,
                Some(s) => matches!(s.status(), ConnectionStatus::Unknown),
            };

            debug!("Reconnect needed: {reconnect_needed}");

            if reconnect_needed {
                if let Some(cfg) = config_signal() {
                    // let mut instance = cfg.into_server(); // returns Box<dyn Server>
                    let mut instance = server::ServerInstance::from_config(cfg);

                    match instance.connect().await {
                        Ok(()) => {
                            debug!("Connected to server: {}", instance.host());
                            // let arc_server: Arc<dyn Server> = instance.into(); // avoid double Arc
                            server_signal.set(Some(Arc::new(instance))); 
                            let _ = nav.push(Route::Home {});
                        }
                        Err(e) => {
                            config_signal.set(None);
                            error!("Connection failed: {e}");
                            nav.push(Route::LoginView {});
                        }
                    }
                } else {
                    nav.push(Route::LoginView {});
                }
            }

            is_ready.set(true);
        }
    });

    if !is_ready() {
        return rsx! { Loading {} };
    }

    rsx! {
        {children}
    }
}

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const MANIFEST: Asset = asset!("/assets/manifest.json");
const SW: Asset = asset!("/assets/sw.js");

pub static APP_HOST: GlobalSignal<utils::AppHost> = GlobalSignal::new(|| utils::AppHost::default());

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
    // use_context_provider(|| {
    //     Signal::new(hooks::AppHost::default())
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
                Router::<Route> {}
            }


        }
}
