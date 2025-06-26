use crate::clients;
use crate::components;
use crate::Route;
use dioxus::logger::tracing::{error, info};
use dioxus::prelude::*;
use remux_web::hooks::use_servers;
use remux_web::server::Jellyfin;
use daisy_rsx;

#[component]
pub fn Settings() -> Element {
    rsx! {
        // components::Hero {}
        Servers{}
       // "heya partner"
    }
}

#[component]
pub fn Servers() -> Element {
    let mut servers = use_servers();

    let t = &*servers.read_unchecked();
    rsx!{
    match t.is_empty() {
        true => rsx! {"No servers"}, // If vector is empty, return this message
        false => {
            rsx! {
                div {
                    class: "carousel w-full",
                    // iterate over the stories with a for loop
                    for server in t {
                        div {
                          "{server.host}"
                        }
                    }
                }
            }
        }
    }
    daisy_rsx::Button {
            Link {
                to: Route::SettingsServersAdd {},
                "Add"
            }
                            }
  }
}


#[component]
pub fn ServersAdd() -> Element {
    let mut servers = use_servers();
    //let server = use_signal(|| Jellyfin::default());
    let server = Jellyfin::default();
    let save_server = move |evt: FormEvent| async move {
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
        
    };


    rsx! {
                div {
                    class: "flex flex-col gap-2",
                        div {
                            class: "form-control bg-base-200 shadow-xl border-0 p-5",

                        form {
                            onsubmit: save_server,
                            div {
                                class: "mb-5",
                            daisy_rsx::Input {
                                name: "name",
                                label: "Name",
                                value: "{server.name}",
                                label_class: "block mb-2 text-sm font-medium",
                                // class: "bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500"
                            }
                            }
                            
                        }
                            daisy_rsx::Button {
                                button_type: daisy_rsx::ButtonType::Submit,
                                "Save",
                            }
              
                        }
                    }
                  
                
          
                    
  }
}