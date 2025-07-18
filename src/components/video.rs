#![allow(non_snake_case)]
use crate::hooks;
use crate::media;
use crate::utils::ResultLogExt;
use anyhow::anyhow;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, error, info, instrument};
use std::{thread, time};
//use dioxus::web::use_eval;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = playShaka)]
    pub fn play_shaka(id: &str, url: &str);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn play_shaka(id: &str, url: &str) {
    // fallback call using eval for desktop/webview
    let call = format!("window.playShaka({:?}, {:?});", id, url);
    document::eval(&call);
}

#[component]
pub fn VideoPlayer() -> Element {
    let mut player = use_video_player();
    // let state = player.read();
    let server = hooks::consume_server().expect("uhu");
    let caps = hooks::use_caps();
    let mut is_loading = use_signal(|| true);

    // let is_visible = *state.visible.read();
    //let media_item = player.media.read().clone().unwrap();
    //let media_source = player.source.read().clone();
    //let status = player.status.read().clone();
    debug!("remder");
    let visible = *player.visible.read();
    let media = player.media.read().clone();
    //let source = player.source.read().clone();

    debug!("render");

    if !visible || media.is_none() {
        debug!("not visible or no media");
        return rsx! {};
    }

    let src = use_resource({
        let server = server.clone();
        let caps = caps.clone();
        move || {
            let media = player.media.read().clone();
            let source = player.source.read().clone();

            let caps = caps.clone();
            let server = server.clone();
            async move { server.get_stream_url(media.unwrap(), source, caps).await }
        }
    });

    let url = match &*src.read() {
        Some(Ok(url)) => url.clone(),
        Some(Err(err)) => {
            error!("stream error: {err}");
            return rsx!(
                div { "error loading stream" }
            );
        }
        None => {
            return rsx! {
                crate::Loading {
                    super::Button {
                        onclick: {
                            to_owned![player];
                            move |_| {
                                player.stop();
                            }
                        },

                        "Cancel"
                    
                    }
                }
            }
        }
    };

    use_effect(move || {
        //is_loading.set(false);
        play_shaka("video-player", &url);
    });

    rsx! {
        div {
            class: "fixed pt-[env(safe-area-inset-top)] inset-0 w-screen h-screen bg-black z-80",
            id: "Gidrocontsinet",
            // Close button
            button {
                class: "absolute pt-[env(safe-area-inset-top)] top-4 right-8 text-white bg-black/70 rounded-full w-10 h-10 flex items-center justify-center hover:bg-black/90",
                aria_label: "Close",
                onclick: {
                    to_owned![player];
                    move |_| {
                        player.stop();
                    }
                },
                svg {
                    xmlns: "http://www.w3.org/2000/svg",
                    fill: "none",
                    view_box: "0 0 24 24",
                    stroke_width: "2",
                    stroke: "currentColor",
                    class: "w-6 h-6",
                    path {
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        d: "M6 18L18 6M6 6l12 12",
                    }
                }
            }
            // Video element with native event handlers
            video {
                class: "w-full h-full object-contain",
                controls: true,
                //preload: "none",
                autoplay: true,
                id: "video-player",
                onplay: {
                    to_owned![player];
                    move |_| {
                        player.play();
                    }
                },
                oncanplay: move |_| {
                    is_loading.set(false);
                },
                onpause: {
                    to_owned![player];
                    move |_| {
                        player.pause();
                    }
                },
                onended: {
                    to_owned![player];
                    move |_| {
                        player.stop();
                    }
                },
            
            // Spinner
            }
            if is_loading() {
                crate::Loading {
                    super::Button {
                        onclick: {
                            to_owned![player];
                            move |_| {
                                player.stop();
                            }
                        },

                        "Cancel"
                    
                    }
                }
            }
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub enum PlaybackStatus {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Stopped,
    Error(String),
}

#[derive(Clone, Default)]
pub struct VideoPlayerState {
    pub status: Signal<PlaybackStatus>,
    pub media: Signal<Option<media::Media>>,
    pub source: Signal<Option<media::MediaSource>>,
    pub visible: Signal<bool>,
}

impl VideoPlayerState {
    pub fn play(&mut self) {
        self.status.set(PlaybackStatus::Playing);
    }

    pub fn pause(&mut self) {
        self.status.set(PlaybackStatus::Paused);
    }

    // #[instrument()]
    pub fn set_media(&mut self, media: media::Media, source: Option<media::MediaSource>) {
        debug!(?media.title, "setting media");
        self.source.set(source);
        self.media.set(Some(media));
        self.status.set(PlaybackStatus::Loading);
        self.visible.set(true);
    }

    pub fn stop(&mut self) {
        debug!("stopping playback");
        self.status.set(PlaybackStatus::Stopped);
        self.visible.set(false);
        self.media.set(None);
        self.source.set(None);
    }
}

pub fn use_video_player() -> VideoPlayerState {
    consume_context()
}
