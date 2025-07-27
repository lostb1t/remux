#![allow(non_snake_case)]
use crate::hooks;
use crate::media;
use crate::sdks;
use crate::utils::ResultLogExt;
use anyhow::anyhow;
use dioxus::prelude::*;
use dioxus_logger::tracing::{debug, error, info, instrument};
use std::{thread, time};
//use dioxus::web::use_eval;
use crate::js_bindings;
use serde::Serialize;

#[derive(Serialize, Debug, Clone)]
pub struct TextTrack {
    pub url: String,
    pub lang: String,
    pub label: String,
    pub mime: Option<String>,
}

impl From<sdks::stremio::Subtitle> for TextTrack {
    fn from(sub: sdks::stremio::Subtitle) -> Self {
        TextTrack {
            url: sub.url,
            lang: sub.lang.clone().unwrap_or_else(|| "und".to_string()), // fallback to "und" (undefined)
            label: sub.lang.clone().unwrap_or_else(|| "Unknown".to_string()),
            mime: Some("text/srt".to_string()),
        }
    }
}

#[component]
pub fn VideoPlayer() -> Element {
    let mut player = use_video_player();
    let server = hooks::use_server()().unwrap();
    let caps = hooks::use_caps();
    let mut is_loading = use_signal(|| true);
    let visible = *player.visible.read();
    let media = player.media.read().clone();
    let media_value = media.clone().unwrap();

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
        to_owned![url, media_value];
        spawn(async move {
            // if *init.read() {
            // let _: () = js_bindings::initShaka("video-player").await.unwrap();
            let text_tracks: Vec<TextTrack> = media_value
                .get_opensubtitles()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|x| x.into())
                .collect();
            //debug!(?text_tracks);
            let _: () = js_bindings::playShaka(url.clone(), text_tracks)
                .await
                .unwrap();
            //  }
            //is_loading.set(false);
        });
    });

    rsx! {
        div {
            class: "fixed pt-[env(safe-area-inset-top)] inset-0 w-screen min-h-screen bg-black z-80",
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
