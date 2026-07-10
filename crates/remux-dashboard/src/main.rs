use dioxus::prelude::*;
use remux_sdks::{
    remux::{
        AuthenticateUserByName, CountryInfo, GetCountries, GetStartupConfiguration,
        JellyfinAuth, PostStartupComplete, PostStartupConfiguration, PostStartupUser,
        PublicSystemInfo, StartupConfiguration, StartupUser, Username,
    },
    ClientError,
};

use crate::state::{
    browser_metadata_country_code, get_or_create_device_id, get_origin,
    get_stored_server, store_credentials, StoredServer, TAILWIND_CSS, THEME_CSS,
};

mod components;
mod layout;
mod pages;
mod router;
mod state;
mod theme;

use crate::theme::use_theme;
use router::Route;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut wizard_needed: Signal<Option<bool>> = use_signal(|| None);
    let mut logged_in = use_signal(|| get_stored_server().is_some());
    use_context_provider(|| logged_in);

    // Initialise theming: applies the persisted mode to <html>, provides the
    // ThemePrefs context, and drives the live accent/scale <style> below.
    let theme = use_theme();

    use_effect(move || {
        spawn(async move {
            let origin = get_origin();
            let needed = match remux_sdks::remux::client(&origin) {
                Ok(c) => c
                    .execute(PublicSystemInfo::default())
                    .await
                    .ok()
                    .map(|info| !info.startup_wizard_completed)
                    .unwrap_or(false),
                Err(_) => false,
            };
            wizard_needed.set(Some(needed));
        });
    });

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: THEME_CSS }
        // Live overrides for the user-chosen accent color and UI scale. A
        // `:root{…}` rule applies globally regardless of the tag's position, and
        // being parsed after the base stylesheet it wins for --accent/--ui-scale.
        style { {crate::theme::theme_style_css(&theme.read())} }
        {match *wizard_needed.read() {
            None => rsx! {
                div { class: "login-page",
                    div { class: "login-card",
                        div { class: "login-header",
                            a { href: "/", class: "login-brand-label", "Remux" }
                            p { class: "connecting", "Starting up…" }
                        }
                    }
                }
            },
            Some(true) => rsx! {
                Wizard {
                    on_complete: move |_| {
                        wizard_needed.set(Some(false));
                    }
                }
            },
            Some(false) => rsx! {
                if *logged_in.read() {
                    Router::<Route> {}
                } else {
                    Login { on_login: move |_| logged_in.set(true) }
                }
            },
        }}
    }
}

#[component]
fn Login(on_login: EventHandler) -> Element {
    let mut server_url: Signal<Option<String>> = use_signal(|| None);
    let mut host_input = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);

    use_effect(move || {
        spawn(async move {
            let origin = get_origin();
            let reachable = match remux_sdks::remux::client(&origin) {
                Ok(c) => c
                    .execute(PublicSystemInfo::default())
                    .await
                    .is_ok(),
                Err(_) => false,
            };
            server_url.set(Some(if reachable { origin } else { String::new() }));
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();

        let url = match server_url
            .peek()
            .clone()
        {
            Some(u) if !u.is_empty() => u,
            _ => {
                let h = host_input
                    .peek()
                    .trim()
                    .to_string();
                if h.is_empty() {
                    error.set(Some("Please enter the server URL".into()));
                    return;
                }
                h
            }
        };

        let u = username
            .peek()
            .clone();
        let p = password
            .peek()
            .clone();
        let device_id = get_or_create_device_id();

        loading.set(true);
        error.set(None);

        spawn(async move {
            let client = match remux_sdks::remux::client(&url) {
                Ok(c) => c.with_auth(JellyfinAuth::new(&device_id)),
                Err(e) => {
                    error.set(Some(format!("Bad server URL: {e}")));
                    loading.set(false);
                    return;
                }
            };

            match client
                .execute(AuthenticateUserByName {
                    username: Some(u),
                    pw: Some(p),
                })
                .await
            {
                Ok(result) => {
                    if let (Some(token), Some(user)) =
                        (result.access_token, result.user)
                    {
                        store_credentials(StoredServer {
                            id: result.server_id,
                            name: "Remux".to_string(),
                            manual_address: url,
                            access_token: token,
                            user_id: user
                                .id
                                .to_string(),
                            date_last_accessed: 0.0,
                        });
                        on_login.call(());
                    } else {
                        error.set(Some("Login failed: no token in response".into()));
                    }
                }
                Err(ClientError::Unauthorized) => {
                    error.set(Some("Invalid username or password".into()));
                }
                Err(e) => {
                    error.set(Some(format!("Login failed: {e}")));
                }
            }

            loading.set(false);
        });
    };

    rsx! {
        div { class: "login-page",
            div { class: "login-card",
                div { class: "login-header",
                    span { class: "login-brand-label", "Remux" }
                    h1 { class: "login-title", "Admin Dashboard" }
                    p { class: "login-subtitle", "Sign in to continue" }
                }
                div { class: "login-body",
                    if server_url.read().is_none() {
                        p { class: "connecting", "Connecting…" }
                    } else {
                        if let Some(err) = error.read().as_ref() {
                            div { class: "alert-error", "{err}" }
                        }

                        form {
                            onsubmit: on_submit,
                            style: "display:flex;flex-direction:column;gap:14px;",

                            if server_url.read().as_deref() == Some("") {
                                div { class: "field",
                                    label { class: "field-label", r#for: "host", "Server URL" }
                                    input {
                                        id: "host",
                                        r#type: "url",
                                        class: "field-input",
                                        placeholder: "http://192.168.1.x:8096",
                                        value: "{host_input}",
                                        oninput: move |e| host_input.set(e.value()),
                                        required: true,
                                    }
                                }
                            }

                            div { class: "field",
                                label { class: "field-label", r#for: "username", "Username" }
                                input {
                                    id: "username",
                                    r#type: "text",
                                    class: "field-input",
                                    value: "{username}",
                                    oninput: move |e| username.set(e.value()),
                                    required: true,
                                    autocomplete: "username",
                                }
                            }
                            div { class: "field",
                                label { class: "field-label", r#for: "password", "Password" }
                                input {
                                    id: "password",
                                    r#type: "password",
                                    class: "field-input",
                                    value: "{password}",
                                    oninput: move |e| password.set(e.value()),
                                    autocomplete: "current-password",
                                }
                            }
                            button {
                                r#type: "submit",
                                class: "btn btn-primary login-btn",
                                disabled: *loading.read(),
                                if *loading.read() { "Signing in…" } else { "Sign In" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn WizardStep(n: u8, label: &'static str, active: bool, done: bool) -> Element {
    let dot_class = if done {
        "wizard-step-dot wizard-step-done"
    } else if active {
        "wizard-step-dot wizard-step-active"
    } else {
        "wizard-step-dot"
    };
    rsx! {
        div { class: "wizard-step",
            div { class: "{dot_class}",
                if done { "✓" } else { "{n}" }
            }
            span { class: "wizard-step-label", "{label}" }
        }
    }
}

#[component]
fn Wizard(on_complete: EventHandler) -> Element {
    let mut step = use_signal(|| 0_u8);
    let mut server_name = use_signal(String::new);
    let mut metadata_country = use_signal(browser_metadata_country_code);
    let mut countries: Signal<Vec<CountryInfo>> = use_signal(Vec::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut password2 = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);

    use_effect(move || {
        let origin = get_origin();
        spawn(async move {
            if let Ok(c) = remux_sdks::remux::client(&origin) {
                if let Ok(cfg) = c
                    .execute(GetStartupConfiguration::default())
                    .await
                {
                    if let Some(name) = cfg
                        .server_name
                        .filter(|s| !s.is_empty())
                    {
                        server_name.set(name);
                    }
                    metadata_country.set(
                        cfg.metadata_country_code
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(browser_metadata_country_code),
                    );
                }
                if let Ok(list) = c
                    .execute(GetCountries)
                    .await
                {
                    countries.set(list);
                }
            }
        });
    });

    rsx! {
        div { class: "wizard-page",
            div { class: "wizard-card",

                div { class: "wizard-steps",
                    WizardStep { n: 1, label: "Server",  active: *step.read() == 0, done: *step.read() > 0 }
                    div { class: "wizard-step-line" }
                    WizardStep { n: 2, label: "Account", active: *step.read() == 1, done: *step.read() > 1 }
                    div { class: "wizard-step-line" }
                    WizardStep { n: 3, label: "Done",    active: *step.read() == 2, done: false }
                }

                div { class: "wizard-header",
                    span { class: "login-brand-label", "Remux" }
                    h2 { class: "wizard-title",
                        {match *step.read() {
                            0 => "Server Configuration",
                            1 => "Create Admin Account",
                            _ => "Setup Complete",
                        }}
                    }
                }

                div { class: "wizard-body",
                    if let Some(err) = error.read().as_ref() {
                        div { class: "alert-error", style: "margin-bottom:16px", "{err}" }
                    }

                    {match *step.read() {

                        0 => rsx! {
                            form {
                                onsubmit: move |e| {
                                    e.prevent_default();
                                    let origin = get_origin();
                                    let name = server_name.peek().clone();
                                    let country = metadata_country.peek().clone();
                                    saving.set(true);
                                    error.set(None);
                                    spawn(async move {
                                        match remux_sdks::remux::client(&origin) {
                                            Ok(c) => match c.execute(PostStartupConfiguration {
                                                payload: StartupConfiguration {
                                                    server_name: Some(name),
                                                    metadata_country_code: Some(country),
                                                    ..Default::default()
                                                },
                                            }).await {
                                                Ok(_)  => step.set(1),
                                                Err(e) => error.set(Some(format!("{e}"))),
                                            },
                                            Err(e) => error.set(Some(format!("Client error: {e}"))),
                                        }
                                        saving.set(false);
                                    });
                                },
                                style: "display:flex;flex-direction:column;gap:16px",

                                p { class: "wizard-desc",
                                    "Give your server a name. Add media addons (Stremio, Deezer, TMDB, …) on the Addons page after setup."
                                }

                                div { class: "field",
                                    label { class: "field-label", r#for: "w-name", "Server Name" }
                                    input {
                                        id: "w-name",
                                        r#type: "text",
                                        class: "field-input",
                                        placeholder: "My Remux Server",
                                        value: "{server_name}",
                                        oninput: move |e| server_name.set(e.value()),
                                    }
                                }

                                div { class: "field",
                                    label { class: "field-label", r#for: "w-country", "Metadata Country" }
                                    select {
                                        id: "w-country",
                                        class: "select-input",
                                        value: "{metadata_country}",
                                        onchange: move |e| metadata_country.set(e.value()),
                                        if countries.read().is_empty() {
                                            option {
                                                value: "{metadata_country}",
                                                selected: true,
                                                "{metadata_country}"
                                            }
                                        }
                                        for country in countries.read().iter() {
                                            option {
                                                value: "{country.two_letter_iso_region_name}",
                                                selected: metadata_country.read().as_str() == country.two_letter_iso_region_name,
                                                "{country.name} ({country.two_letter_iso_region_name})"
                                            }
                                        }
                                    }
                                    p { class: "field-hint",
                                        "Used for metadata ratings and regional release details."
                                    }
                                }

                                div { class: "wizard-actions",
                                    button {
                                        r#type: "submit",
                                        class: "btn btn-primary",
                                        disabled: *saving.read(),
                                        if *saving.read() { "Saving…" } else { "Next →" }
                                    }
                                }
                            }
                        },

                        1 => rsx! {
                            form {
                                onsubmit: move |e| {
                                    e.prevent_default();
                                    let origin = get_origin();
                                    let name = username.peek().clone();
                                    let pw   = password.peek().clone();
                                    let pw2  = password2.peek().clone();
                                    let name = match Username::try_new(name) {
                                        Ok(u) => u,
                                        Err(_) => {
                                            error.set(Some("Invalid username: must contain only letters, digits, spaces, and -'._@+, and be at most 255 characters".into()));
                                            return;
                                        }
                                    };
                                    if pw != pw2 {
                                        error.set(Some("Passwords do not match".into()));
                                        return;
                                    }
                                    saving.set(true);
                                    error.set(None);
                                    spawn(async move {
                                        match remux_sdks::remux::client(&origin) {
                                            Ok(c) => match c.execute(PostStartupUser {
                                                payload: StartupUser {
                                                    name: Some(name),
                                                    password: Some(pw.clone()),
                                                    password_confirm: Some(pw),
                                                },
                                            }).await {
                                                Ok(_)  => step.set(2),
                                                Err(e) => error.set(Some(format!("{e}"))),
                                            },
                                            Err(e) => error.set(Some(format!("Client error: {e}"))),
                                        }
                                        saving.set(false);
                                    });
                                },
                                style: "display:flex;flex-direction:column;gap:16px",

                                p { class: "wizard-desc",
                                    "Create the administrator account you will use to log in."
                                }

                                div { class: "field",
                                    label { class: "field-label", r#for: "w-user", "Username" }
                                    input {
                                        id: "w-user",
                                        r#type: "text",
                                        class: "field-input",
                                        required: true,
                                        value: "{username}",
                                        oninput: move |e| username.set(e.value()),
                                        autocomplete: "username",
                                    }
                                }
                                div { class: "field",
                                    label { class: "field-label", r#for: "w-pw", "Password" }
                                    input {
                                        id: "w-pw",
                                        r#type: "password",
                                        class: "field-input",
                                        required: true,
                                        value: "{password}",
                                        oninput: move |e| password.set(e.value()),
                                        autocomplete: "new-password",
                                    }
                                }
                                div { class: "field",
                                    label { class: "field-label", r#for: "w-pw2", "Confirm Password" }
                                    input {
                                        id: "w-pw2",
                                        r#type: "password",
                                        class: "field-input",
                                        required: true,
                                        value: "{password2}",
                                        oninput: move |e| password2.set(e.value()),
                                        autocomplete: "new-password",
                                    }
                                }

                                div { class: "wizard-actions wizard-actions-split",
                                    button {
                                        r#type: "button",
                                        class: "btn btn-ghost",
                                        onclick: move |_| { error.set(None); step.set(0); },
                                        "← Back"
                                    }
                                    button {
                                        r#type: "submit",
                                        class: "btn btn-primary",
                                        disabled: *saving.read(),
                                        if *saving.read() { "Creating…" } else { "Next →" }
                                    }
                                }
                            }
                        },

                        _ => rsx! {
                            div { style: "display:flex;flex-direction:column;gap:20px",
                                p { class: "wizard-desc",
                                    "Your server is configured and the admin account has been created. "
                                    "Click Finish to complete setup and go to the login page."
                                }
                                div { class: "wizard-actions",
                                    button {
                                        class: "btn btn-primary",
                                        style: "width:100%",
                                        disabled: *saving.read(),
                                        onclick: move |_| {
                                            let origin = get_origin();
                                            saving.set(true);
                                            error.set(None);
                                            spawn(async move {
                                                if let Ok(c) = remux_sdks::remux::client(&origin) {
                                                    let _ = c.execute(PostStartupComplete::default()).await;
                                                }
                                                on_complete.call(());
                                            });
                                        },
                                        if *saving.read() { "Finishing…" } else { "Finish Setup" }
                                    }
                                }
                            }
                        },
                    }}
                }
            }
        }
    }
}
