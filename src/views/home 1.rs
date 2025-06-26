use std::time::Duration;

use crate::clients;
use crate::components;
use dioxus::prelude::*;
use eyre::Result;
// use jellyfin_api;
use daisy_rsx;
use dioxus_logger::tracing::{debug, info};
use jellyfin_api;
use remux_web::server::Jellyfin;
use remux_web::server::Session;
use remux_web::{hooks::*, server::Server};
use tokio_with_wasm::alias as tokio;

#[component]
pub fn Home() -> Element {
    rsx! {
        // components::Hero {}
        // components::Button { "Click me" }
        Media {}
    }
}

#[component]
fn ServerSessions(server: Jellyfin) -> Element {
    let mut sessions: Signal<Vec<Session>> = use_signal(|| vec![]);

    // let _: Resource<Result<Vec<Session>>> = use_resource(move || async move {
    let _ = use_resource(move || {
        //to_owned![server];
        let s = server.clone();
        async move {
            loop {
                tokio::time::sleep(Duration::from_secs(4)).await;
                let new_sessions: Vec<Session> = s
                    .sessions()
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|x| x.media.is_some())
                    .collect();
                sessions.set(new_sessions);
                //return Ok(sessions)
            }
        }
    });
    let list = &*sessions.read_unchecked();
    // match &*sessions.read_unchecked() {
    //     Some(list) => {
    match list.is_empty() {
        true => rsx! {"Nothing playing"},
        false => {
            rsx! {
                div {
                    class: "carousel w-full",
                    for session in list {
                        SessionDetail{server: server.clone(), session: session.clone()}
                    }
                }
            }
        }
    }
}

#[component]
fn SessionDetail(server: Jellyfin, session: Session) -> Element {
  let media = session.media.clone().unwrap();
  rsx! {
                        img {
                          src: "{server.poster_url(media}"
                        }
                        div {
                            class: "carousel-item",
                            daisy_rsx::Card {
                                class: "w-full m-4",
                                daisy_rsx::CardHeader {
                                    class: "text-sm",
                                    title: "{media.name}"
                                }
                            }
                        }
                      }
}

#[component]
fn Media() -> Element {
    let mut app = use_app();
    let mut servers = use_servers();
    //let mut sessions: Signal<Option<Vec<Session>>> = use_signal(|| None);
    she
    //info!("{:?}", &servers.0.host());
    // Fetch the top 10 stories on Hackernews
    //let media = use_resource(move || clients::remux::get_media());
    // let media: Resource<Result<Vec<Session>>> = use_resource(move || async move {
    //     loop {
    //         tokio::+time::sleep(Duration::from_secs(4)).await;
    //         info!("Fetching sessions");
    //         let mut sessions: Vec<Session> = vec![];
    //         for server in servers.iter() {
    //             sessions.extend(server.sessions().await.unwrap());
    //         }
    //         return Ok(sessions)
    //     }
    // })

    //let media: Resource<Result<Vec<Session>>> = use_resource(move || async move {
    //    loop {
    //        tokio::time::sleep(Duration::from_secs(4)).await;
    //        debug!("Fetching sessions");
    //        let mut s: Vec<Session> = vec![];
    //        for server in servers.iter() {
    //            s.extend(server.sessions().await.unwrap().into_iter().filter(|x| x.media.is_some()));
    //        }
    //        sessions.set(Some(s));
    //return Ok(sessions)
    //    }
    //});

    // let media = use_resource(move || jellyfin_api::Client);
    let t = &*servers.read_unchecked();
    // check if the future is resolved
    // match &*media.read_unchecked() {
    //    match &*servers.read_unchecked() {
    //  Some(s) => {

    match t.is_empty() {
        true => rsx! {"Nothing servers"}, // If vector is empty, return this message
        false => {
            rsx! {
                div {
                    class: "carousel w-full",
                    // iterate over the stories with a for loop
                    for server in t {
                        ServerSessions {server: server.clone()}
                    }
                }
            }
        }
    }
    // }
    //   }
}
