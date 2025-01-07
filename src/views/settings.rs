use crate::clients;
use crate::components;
use dioxus::logger::tracing::{error, info};
use dioxus::prelude::*;

use daisy_rsx;

#[component]
pub fn Settings() -> Element {
    rsx! {
        // components::Hero {}
        Sources{}
    }
}

fn Sources() -> Element {
    let sources = use_resource(move || clients::remux::get_sources());

    let save_source = move |evt: FormEvent| async move {
        let data = evt.values();
        let source = clients::remux::Source {
            id: data.get("id").unwrap().as_value().parse().unwrap(),
            name: data.get("name").unwrap().as_value(),
            enable: data.get("enable").is_some(),
            service_type: data
                .get("service_type")
                .unwrap()
                .as_value()
                .parse()
                .unwrap(),
        };
        // info!("Saving new source: {:?}", &data);
        match clients::remux::update_source(source).await {
            Ok(_) => info!("Source saved successfully"),
            Err(err) => error!("Error saving source: {:?}", err),
        }
    };

    // check if the future is resolved
    match &*sources.read_unchecked() {
        Some(Ok(list)) => {
            // if it is, render the stories
            rsx! {
                div {
                    class: "flex flex-col gap-2",
                    for source in list {
                        div {
                            class: "form-control bg-base-200 shadow-xl border-0 p-5",

                        form {
                            onsubmit: save_source,
                            input {
                                type: "hidden",
                                name: "id",
                                value: "{source.id}",
                            }
                            div {
                                class: "mb-5",
                            daisy_rsx::Input {
                                name: "name",
                                label: "Name",
                                value: "{source.name}",
                                label_class: "block mb-2 text-sm font-medium",
                                // class: "bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500"
                            }
                            }
                            div {
                                class: "mb-5",
                            daisy_rsx::Select {
                                name: "service_type",
                                label: "Service Type",
                                value: "{source.service_type}",
                                // for option in clients::remux::ServiceType {
                                    daisy_rsx::SelectOption {
                                        value: "{clients::remux::ServiceType::Fs}",
                                        "{clients::remux::ServiceType::Fs}"
                                    }
                                // }
                            }
                            }
                            div {
                                class: "mb-5",
                            daisy_rsx::CheckBox {
                                name: "enable",
                                value: "enable",
                                checked: source.enable
                            }
                        }
                            daisy_rsx::Button {
                                button_type: daisy_rsx::ButtonType::Submit,
                                "Save",
                            }
                            daisy_rsx::Button {
                                "Delete"
                            }
                        }
                    }
                    }
                }
            }
        }
        Some(Err(err)) => {
            // if there was an error, render the error
            rsx! {"An error occurred while fetching sources {err}"}
        }
        None => {
            // if the future is not resolved yet, render a loading message
            rsx! {"Loading sources"}
        }
    }
}
