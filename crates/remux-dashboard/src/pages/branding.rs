use crate::{
    components::{Card, ErrorAlert, LoadingText, SuccessAlert, ToggleRow},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    BrandingOptions, BrandingScriptInfo, GetBrandingConfiguration, GetBrandingScripts,
    RemuxBrandingExtensions, UpdateBrandingConfiguration,
};

#[component]
pub fn BrandingPage(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<BrandingOptions>> = use_signal(|| None);
    let mut custom_css = use_signal(String::new);
    let mut custom_js = use_signal(String::new);
    let mut enabled_scripts: Signal<Vec<String>> = use_signal(Vec::new);
    let mut available_scripts: Signal<Vec<BrandingScriptInfo>> = use_signal(Vec::new);
    let mut login_disclaimer = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load.clone();
        spawn(async move {
            match client
                .execute(GetBrandingConfiguration)
                .await
            {
                Ok(cfg) => {
                    custom_css.set(
                        cfg.custom_css
                            .clone()
                            .unwrap_or_default(),
                    );
                    custom_js.set(
                        cfg.remux
                            .as_ref()
                            .and_then(|r| {
                                r.custom_js
                                    .clone()
                            })
                            .unwrap_or_default(),
                    );
                    enabled_scripts.set(
                        cfg.remux
                            .as_ref()
                            .map(|r| {
                                r.enabled_scripts
                                    .clone()
                            })
                            .unwrap_or_default(),
                    );
                    login_disclaimer.set(
                        cfg.login_disclaimer
                            .clone()
                            .unwrap_or_default(),
                    );
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load branding: {e}"))),
            }
            loading.set(false);
        });
    });

    // Catalog comes from the server so the list never drifts from the build.
    let app_state_scripts = app_state.clone();
    use_effect(move || {
        let client = app_state_scripts.clone();
        spawn(async move {
            if let Ok(scripts) = client
                .execute(GetBrandingScripts)
                .await
            {
                available_scripts.set(scripts);
            }
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state.clone();
        let css = custom_css
            .peek()
            .clone();
        let js = custom_js
            .peek()
            .clone();
        let disc = login_disclaimer
            .peek()
            .clone();

        let mut cfg = base_cfg
            .peek()
            .clone()
            .unwrap_or_default();
        cfg.custom_css = if css.is_empty() { None } else { Some(css) };
        cfg.login_disclaimer = if disc.is_empty() { None } else { Some(disc) };
        cfg.remux = Some(RemuxBrandingExtensions {
            custom_js: if js.is_empty() { None } else { Some(js) },
            enabled_scripts: enabled_scripts
                .peek()
                .clone(),
        });

        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateBrandingConfiguration { config: cfg })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        Card { title: "Branding",
            if *loading.read() {
                LoadingText {}
            } else {
                form {
                    onsubmit: on_submit,
                    style: "display:flex;flex-direction:column;gap:14px",

                        div { class: "field",
                            label { class: "field-label", r#for: "b-css", "Custom CSS" }
                            p { class: "field-hint", "Injected into every page of the Jellyfin web client." }
                            textarea {
                                id: "b-css",
                                class: "field-input",
                                style: "min-height:220px;resize:vertical;font-family:var(--font-mono);font-size:.78rem",
                                value: "{custom_css}",
                                oninput: move |e| custom_css.set(e.value()),
                            }
                        }

                        if !available_scripts.read().is_empty() {
                            div { class: "field",
                                label { class: "field-label", "Built-in scripts" }
                                p { class: "field-hint",
                                    "Optional behaviour bundled with the server. Enabled scripts are injected before Custom JS, so anything you write below still takes precedence."
                                }
                                for script in available_scripts.read().clone() {
                                    ToggleRow {
                                        key: "{script.id}",
                                        label: script.name.clone(),
                                        description: script.description.clone(),
                                        checked: enabled_scripts.read().contains(&script.id),
                                        on_change: {
                                            let id = script.id.clone();
                                            move |wanted: bool| {
                                                let id = id.clone();
                                                enabled_scripts.with_mut(|list| {
                                                    if wanted {
                                                        if !list.contains(&id) {
                                                            list.push(id);
                                                        }
                                                    } else {
                                                        list.retain(|s| s != &id);
                                                    }
                                                });
                                            }
                                        },
                                    }
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "b-js", "Custom JS" }
                            p { class: "field-hint", "Injected into every page of the Jellyfin web client." }
                            textarea {
                                id: "b-js",
                                class: "field-input",
                                style: "min-height:220px;resize:vertical;font-family:var(--font-mono);font-size:.78rem",
                                value: "{custom_js}",
                                oninput: move |e| custom_js.set(e.value()),
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "b-disc", "Login Disclaimer" }
                            p { class: "field-hint", "Text shown below the login form." }
                            textarea {
                                id: "b-disc",
                                class: "field-input",
                                style: "min-height:80px;resize:vertical",
                                value: "{login_disclaimer}",
                                oninput: move |e| login_disclaimer.set(e.value()),
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            ErrorAlert { message: err.clone() }
                        }
                        if *saved.read() {
                            SuccessAlert { message: "Branding saved.".to_string() }
                        }

                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save" }
                            }
                        }
                    }
                }
        }
    }
}
