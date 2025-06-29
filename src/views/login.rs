use dioxus::prelude::*;
use dioxus::signals::*;

#[component]
pub fn LoginView() -> Element {
    let mut host = use_signal(|| String::new());
    let mut username = use_signal(|| String::new());
    let mut password = use_signal(|| String::new());

    let on_login = move |_| {
        println!(
            "Logging in to {} as {} with password {}",
            host(), username(), password()
        );
        // TODO: perform login request here
    };

    rsx! {
        div {
            class: "min-h-screen flex flex-col items-center justify-center bg-gray-100",
            div {
                class: "bg-white p-8 rounded shadow-md w-full max-w-sm",
                h1 {
                    class: "text-2xl font-bold mb-6 text-center",
                    "Login"
                }

                input {
                    r#type: "text",
                    placeholder: "Server URL",
                    class: "mb-4 w-full px-3 py-2 border border-gray-300 rounded",
                    value: "{host}",
                    oninput: move |e| host.set(e.value().clone())
                }

                input {
                    r#type: "text",
                    placeholder: "Username",
                    class: "mb-4 w-full px-3 py-2 border border-gray-300 rounded",
                    value: "{username}",
                    oninput: move |e| username.set(e.value().clone())
                }

                input {
                    r#type: "password",
                    placeholder: "Password",
                    class: "mb-6 w-full px-3 py-2 border border-gray-300 rounded",
                    value: "{password}",
                    oninput: move |e| password.set(e.value().clone())
                }

                button {
                    onclick: on_login,
                    class: "w-full bg-blue-500 text-white px-4 py-2 rounded hover:bg-blue-600",
                    "Log In"
                }
            }
        }
    }
}