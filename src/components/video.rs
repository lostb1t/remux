#![allow(non_snake_case)]
use dioxus::prelude::*;
use std::{thread, time};
//use dioxus::web::use_eval;

#[derive(PartialEq, Props, Clone)]
pub struct VideoPlayerProps {
    pub src: String,
  //  pub r#type: String,
    // pub fullscreen: true,
}

#[component]
pub fn VideoPlayer(props: VideoPlayerProps) -> Element {
    let mut player = use_video_player();
    rsx! {
div {
class: "fixed inset-0 bg-black z-50",
            div {
                class: "absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2",

            video {
                id: "videoplayer",
                controls: true,
                preload: "none",
                autoplay: false,
                poster: "https://vjs.zencdn.net/v/oceans.png",
                source { src: "{props.src}" }
            }
            button {
                class: "btn mb-10",
                onclick: move |_| async move {
                  player.with_mut(|s| s.src = None)
                },
                "Close"
            }
            button {
                class: "btn mb-10",
                onclick: move |_| async move {
                    let mut eval = document::eval(
                            r#"
                                                                        function fullScreen() {
                                                                            //let elem = document.querySelector("video");
                                                                            let elem = document.querySelector('#videoplayer');
                                                                            if (elem) {
                                                                                var requestFullScreen = elem.requestFullscreen || elem.webkitRequestFullscreen || elem.webkitEnterFullscreen || elem.mozRequestFullScreen ||  elem.msRequestFullscreen;
                                                                                // requestFullScreen();
                                                                                requestFullScreen.call(elem);
                                                                            }
                                                                        };
                                                                        fullScreen();
                                                                        //
                                                                        "#,
                        )
                        .await.unwrap();
                },
                "Fullscreen"
            }
        }
    }
  }
}

#[derive(Clone, Default)]
pub struct VideoPlayerState {
   // pub media: Option<Media>,w
   // pub stream: Option<MediaStream>,
    pub state: Option<String>,
    pub src: Option<String>
}

pub fn use_video_player() -> Signal<VideoPlayerState> {
    use_context::<Signal<VideoPlayerState>>()
}

#[component]
pub fn VideoPlayerCallback() -> Element {
    let state = use_video_player();
    // debug!("{:?}", video_player_state.read());
    //let state = use_signal::<VideoPlayerState>(|| VideoPlayerState::default());
    //0let state = video_player_state.read();
    
    // let create_eval = use_eval(cx);
    // let mut eval = create_eval(
    //     r#"
    //     console.log('whats the element');
    //     function myFunction() {
    //         // let elem = document.getElementById("videoplayer");
    //         let elem = document.querySelector("video");
    //         console.log('running function');
    //         if (elem) {
    //             console.log(elem);
    //             var requestFullScreen = elem.requestFullscreen || elem.webkitRequestFullscreen || elem.webkitEnterFullscreen || elem.mozRequestFullScreen ||  elem.msRequestFullscreen;
    //             //console.log(requestFullScreen);
    //             requestFullScreen();
    //         }
    //     };
    //     myFunction();
    //     // elem.requestFullscreen();
    //     "#,
    // )
    // .unwrap();
    // dbg!("rerendering");

    // rsx! {
    //     VideoPlayer {
    //         src: "https://devstreaming-cdn.apple.com/videos/streaming/examples/img_bipbop_adv_example_fmp4/master.m3u8".to_string(),
    //         r#type: "application/x-mpegURL".to_string()
    //     }
    // }
    if state.read().src.is_some() {
        rsx! {
            VideoPlayer {
                src: "https://devstreaming-cdn.apple.com/videos/streaming/examples/img_bipbop_adv_example_fmp4/master.m3u8"
                    .to_string(),
                //r#type: "application/x-mpegURL".to_string()
            }
        }
        
    } else {
        rsx! {}
    }
    // rsx! {
    //     video {
    //         id: "player",
    //         controls: true,
    //         // crossorigin: "use-credentials",
    //         // playsinline: true,
    //         preload: "none",
    //         // responsive is possible: https://github.com/videojs/video.js/blob/main/sandbox/responsive.html.example bit sucky
    //         class: "video-js",
    //         poster: "https://vjs.zencdn.net/v/oceans.png",
    //         "data-setup": "{{}}",
    //         // source {
    //         //     src: "https://devstreaming-cdn.apple.com/videos/streaming/examples/img_bipbop_adv_example_fmp4/master.m3u8",
    //         //     r#type: "application/x-mpegURL"
    //         // }
    //         source {
    //            // src: "https://plex.sjoerdarendsen.dev/video/:/transcode/universal/start.m3u8?X-Plex-Token=U2_Qf8WFz5wT-tN_hdfx&X-Plex-Session-Identifier=r6gj84598it3vk0bt98y9ufo&path=/library/metadata/1085862&X-Plex-Platform=Generic&protocol=hls",
    //             src: "{video_player_state.state}",
    //             // r#type: "{cx.props.r#type}"
    //         }
    //     }
    // }
}