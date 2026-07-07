use crate::{
    components::{Card, ErrorAlert, FormActions, LoadingText, SuccessAlert, ToggleRow},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    CountryInfo, EmbeddedSubtitleHandling, EncodingOptions, GetCountries,
    GetEncodingConfiguration, GetIntroConfiguration, GetSystemConfiguration,
    HardwareAccelerationType, IntroOptions, IntroOrder, IntroTriggers,
    ServerConfiguration, StartTask, UpdateEncodingConfiguration,
    UpdateIntroConfiguration, UpdateSystemConfiguration,
};

#[component]
pub fn ServerSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut server_name = use_signal(String::new);
    let mut metadata_country = use_signal(|| "US".to_string());
    let mut countries: Signal<Vec<CountryInfo>> = use_signal(Vec::new);
    let mut catalog_max_items = use_signal(|| 100_i64);
    let mut meta_concurrency = use_signal(|| 12_i64);
    let mut filter_digital_release = use_signal(|| true);
    let mut digital_release_buffer = use_signal(|| 0_i64);
    let mut subtitle_languages = use_signal(String::new);
    let mut quick_connect_enabled = use_signal(|| true);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetSystemConfiguration)
                .await
            {
                Ok(cfg) => {
                    server_name.set(
                        cfg.server_name
                            .clone()
                            .unwrap_or_default(),
                    );
                    metadata_country.set(
                        cfg.metadata_country_code
                            .clone()
                            .unwrap_or_else(|| "US".to_string()),
                    );
                    catalog_max_items.set(
                        cfg.catalog_max_items
                            .unwrap_or(100),
                    );
                    meta_concurrency.set(cfg.meta_concurrency);
                    filter_digital_release.set(cfg.filter_by_digital_release_date);
                    digital_release_buffer.set(cfg.digital_release_buffer_days);
                    subtitle_languages.set(
                        cfg.subtitle_languages
                            .as_deref()
                            .map(|v| v.join(", "))
                            .unwrap_or_default(),
                    );
                    quick_connect_enabled.set(
                        cfg.quick_connect_available
                            .unwrap_or(true),
                    );
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load settings: {e}"))),
            }
            if let Ok(list) = client
                .execute(GetCountries)
                .await
            {
                countries.set(list);
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state
            .client
            .clone();
        let name = server_name
            .peek()
            .clone();
        let country = metadata_country
            .peek()
            .clone();
        let max = *catalog_max_items.peek();
        let concurrency = *meta_concurrency.peek();
        let filter_dr = *filter_digital_release.peek();
        let dr_buffer = *digital_release_buffer.peek();
        let sub_langs_str = subtitle_languages
            .peek()
            .clone();
        let qc_enabled = *quick_connect_enabled.peek();

        let mut cfg = base_cfg
            .peek()
            .clone()
            .unwrap_or_default();
        cfg.server_name = Some(name);
        cfg.metadata_country_code = Some(country);
        cfg.quick_connect_available = Some(qc_enabled);
        cfg.catalog_max_items = Some(max);
        cfg.meta_concurrency = concurrency;
        cfg.filter_by_digital_release_date = filter_dr;
        cfg.digital_release_buffer_days = dr_buffer;
        cfg.subtitle_languages = Some(
            sub_langs_str
                .split(',')
                .map(|s| {
                    s.trim()
                        .to_lowercase()
                })
                .filter(|s| !s.is_empty())
                .collect(),
        );

        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: cfg })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        Card { title: "General Settings",
            if *loading.read() {
                LoadingText {}
            } else {
                form {
                        onsubmit: on_submit,
                        style: "display:flex;flex-direction:column;gap:14px",

                        div { class: "field",
                            label { class: "field-label", r#for: "s-name", "Server Name" }
                            input {
                                id: "s-name",
                                r#type: "text",
                                class: "field-input",
                                value: "{server_name}",
                                oninput: move |e| server_name.set(e.value()),
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "s-country", "Metadata Country" }
                            select {
                                id: "s-country",
                                class: "select-input",
                                value: "{metadata_country}",
                                onchange: move |e| metadata_country.set(e.value()),
                                for country in countries.read().iter() {
                                    option {
                                        value: "{country.two_letter_iso_region_name}",
                                        selected: metadata_country.read().as_str() == country.two_letter_iso_region_name,
                                        "{country.name} ({country.two_letter_iso_region_name})"
                                    }
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "s-max", "Catalog Max Items" }
                            input {
                                id: "s-max",
                                r#type: "number",
                                class: "field-input",
                                min: "1",
                                value: "{catalog_max_items}",
                                oninput: move |e| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        catalog_max_items.set(n);
                                    }
                                },
                            }
                            p { class: "field-hint",
                                "Maximum number of items imported per collection."
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "s-concurrency", "Metadata Concurrency" }
                            input {
                                id: "s-concurrency",
                                r#type: "number",
                                class: "field-input",
                                min: "1",
                                max: "200",
                                value: "{meta_concurrency}",
                                oninput: move |e| {
                                    if let Ok(n) = e.value().parse::<i64>() {
                                        meta_concurrency.set(n);
                                    }
                                },
                            }
                            p { class: "field-hint",
                                "Number of items to enrich with metadata concurrently during library import. Higher values are faster but increase memory usage and may trigger rate limits on metadata sources. Default: 12."
                            }
                        }

                        div { class: "field",
                            label { class: "field-label",
                                input {
                                    r#type: "checkbox",
                                    checked: *filter_digital_release.read(),
                                    oninput: move |e| filter_digital_release.set(e.checked()),
                                }
                                " Filter by digital release date"
                            }
                            p { class: "field-hint",
                                "Hide items that haven't been digitally released yet. Items released theatrically within the past year are always hidden when no digital date is available."
                            }
                        }

                        if *filter_digital_release.read() {
                            div { class: "field",
                                label { class: "field-label", r#for: "s-dr-buf", "Release buffer (days)" }
                                input {
                                    id: "s-dr-buf",
                                    r#type: "number",
                                    class: "field-input",
                                    min: "0",
                                    max: "365",
                                    value: "{digital_release_buffer}",
                                    oninput: move |e| {
                                        if let Ok(n) = e.value().parse::<i64>() {
                                            digital_release_buffer.set(n);
                                        }
                                    },
                                }
                                p { class: "field-hint",
                                    "Show items releasing up to this many days in the future. 0 = today or earlier only."
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "s-sub-langs", "Subtitle Languages" }
                            input {
                                id: "s-sub-langs",
                                r#type: "text",
                                class: "field-input",
                                placeholder: "en, de, fr",
                                value: "{subtitle_languages}",
                                oninput: move |e| subtitle_languages.set(e.value()),
                            }
                            p { class: "field-hint",
                                "Comma-separated ISO 639-1 codes (e.g. \"en, de\"). "
                                "Only subtitles in these languages are shown and the first match is selected by default. "
                                "Leave empty to show all subtitles without a default."
                            }
                        }

                        div { class: "field",
                            label { class: "field-label",
                                input {
                                    r#type: "checkbox",
                                    checked: *quick_connect_enabled.read(),
                                    oninput: move |e| quick_connect_enabled.set(e.checked()),
                                }
                                " Enable QuickConnect"
                            }
                            p { class: "field-hint",
                                "Allow clients to log in by entering a code shown on the login screen."
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            ErrorAlert { message: err.clone() }
                        }
                        if *saved.read() {
                            SuccessAlert { message: "Settings saved.".to_string() }
                        }

                        FormActions {
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save Settings" }
                            }
                        }
                    }
                }
        }
    }
}

#[component]
pub fn PlaybackSettingsCard(app_state: AppState) -> Element {
    let mut encoding_preset = use_signal(|| "ultrafast".to_string());
    let mut hw_accel = use_signal(|| "none".to_string());
    let mut auto_detect = use_signal(|| true);
    let mut enable_tonemapping = use_signal(|| false);
    let mut enable_vpp_tonemapping = use_signal(|| false);
    let mut tonemapping_algorithm = use_signal(|| "hable".to_string());
    let mut tonemapping_desat = use_signal(|| 0.0_f32);
    let mut tonemapping_peak = use_signal(|| 0.0_f32);
    let mut allow_hevc_encoding = use_signal(|| false);
    let mut allow_av1_encoding = use_signal(|| false);
    let mut h264_crf = use_signal(|| 23_u32);
    let mut h265_crf = use_signal(|| 28_u32);
    let mut enable_video_transcoding = use_signal(|| true);
    let mut subtitle_mode = use_signal(|| "Burn".to_string());
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetEncodingConfiguration)
                .await
            {
                Ok(opts) => {
                    encoding_preset.set(
                        opts.encoding_preset
                            .unwrap_or_default()
                            .to_string(),
                    );
                    auto_detect.set(
                        opts.auto_detect_hardware_acceleration
                            .unwrap_or(true),
                    );
                    let accel_str = match opts
                        .hardware_acceleration_type
                        .unwrap_or_default()
                    {
                        HardwareAccelerationType::None => "none",
                        HardwareAccelerationType::Vaapi => "vaapi",
                        HardwareAccelerationType::Nvenc => "nvenc",
                        HardwareAccelerationType::Qsv => "qsv",
                        HardwareAccelerationType::Amf => "amf",
                        HardwareAccelerationType::VideoToolbox => "videotoolbox",
                        HardwareAccelerationType::V4l2m2m => "v4l2m2m",
                        HardwareAccelerationType::Rkmpp => "rkmpp",
                    };
                    hw_accel.set(accel_str.to_string());
                    enable_tonemapping.set(
                        opts.enable_tonemapping
                            .unwrap_or(false),
                    );
                    enable_vpp_tonemapping.set(
                        opts.enable_vpp_tonemapping
                            .unwrap_or(false),
                    );
                    tonemapping_algorithm.set(
                        opts.tonemapping_algorithm
                            .unwrap_or_else(|| "hable".to_string()),
                    );
                    tonemapping_desat.set(
                        opts.tonemapping_desat
                            .unwrap_or(0.0),
                    );
                    tonemapping_peak.set(
                        opts.tonemapping_peak
                            .unwrap_or(0.0),
                    );
                    allow_hevc_encoding.set(
                        opts.allow_hevc_encoding
                            .unwrap_or(true),
                    );
                    allow_av1_encoding.set(
                        opts.allow_av1_encoding
                            .unwrap_or(false),
                    );
                    h264_crf.set(
                        opts.h264_crf
                            .unwrap_or(23),
                    );
                    h265_crf.set(
                        opts.h265_crf
                            .unwrap_or(28),
                    );
                    enable_video_transcoding.set(
                        opts.enable_video_transcoding
                            .unwrap_or(true),
                    );
                    subtitle_mode.set(
                        opts.subtitle_mode
                            .unwrap_or(EmbeddedSubtitleHandling::Burn)
                            .to_string(),
                    );
                }
                Err(e) => error.set(Some(format!("Failed to load settings: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state
            .client
            .clone();
        let accel_type = match hw_accel
            .peek()
            .as_str()
        {
            "vaapi" => HardwareAccelerationType::Vaapi,
            "nvenc" => HardwareAccelerationType::Nvenc,
            "qsv" => HardwareAccelerationType::Qsv,
            "amf" => HardwareAccelerationType::Amf,
            "videotoolbox" => HardwareAccelerationType::VideoToolbox,
            "v4l2m2m" => HardwareAccelerationType::V4l2m2m,
            "rkmpp" => HardwareAccelerationType::Rkmpp,
            _ => HardwareAccelerationType::None,
        };
        let opts = EncodingOptions {
            encoding_preset: encoding_preset
                .peek()
                .parse()
                .ok(),
            hardware_acceleration_type: Some(accel_type),
            vaapi_device: None,
            vaapi_driver: None,
            auto_detect_hardware_acceleration: Some(*auto_detect.peek()),
            enable_tonemapping: Some(*enable_tonemapping.peek()),
            enable_vpp_tonemapping: Some(*enable_vpp_tonemapping.peek()),
            tonemapping_algorithm: Some(
                tonemapping_algorithm
                    .peek()
                    .clone(),
            ),
            tonemapping_desat: Some(*tonemapping_desat.peek()),
            tonemapping_peak: Some(*tonemapping_peak.peek()),
            allow_hevc_encoding: Some(*allow_hevc_encoding.peek()),
            allow_av1_encoding: Some(*allow_av1_encoding.peek()),
            h264_crf: Some(*h264_crf.peek()),
            h265_crf: Some(*h265_crf.peek()),
            enable_video_transcoding: Some(*enable_video_transcoding.peek()),
            subtitle_mode: subtitle_mode
                .peek()
                .parse::<EmbeddedSubtitleHandling>()
                .ok(),
        };
        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateEncodingConfiguration { config: opts })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        Card { title: "Playback",
            if *loading.read() {
                LoadingText {}
            } else {
                form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                        div { class: "field",
                            label { class: "field-label", "Video Transcoding" }
                            div { class: "field-hint",
                                "Allow the server to re-encode video streams. When disabled, the video track is always copied as-is (remux). Remuxing and audio transcoding are always available regardless of this setting."
                            }
                            label { style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *enable_video_transcoding.read(),
                                    onchange: move |e| enable_video_transcoding.set(e.checked()),
                                }
                                "Enable video transcoding"
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "Unsupported Subtitle Handling" }
                            div { class: "field-hint",
                                "What to do with embedded subtitle streams the client device doesn't support. Burn encodes them into the video. Extract delivers them separately via the subtitle stream endpoint (may be slow for remote sources). Strip removes them from the media source so the client never sees them — no subtitle-triggered transcoding."
                            }
                            select {
                                class: "select-input",
                                value: subtitle_mode.read().clone(),
                                onchange: move |e| subtitle_mode.set(e.value()),
                                option { value: "Burn", "Burn into video (default)" }
                                option { value: "Extract", "Extract and deliver separately" }
                                option { value: "Strip", "Strip (remove, no transcoding)" }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "Hardware Acceleration" }
                            div { class: "field-hint",
                                "GPU-accelerated video encoding. When auto-detect is on, the server probes available hardware at startup and selects the best option."
                            }
                            label { style: "display:flex;align-items:center;gap:8px;margin-bottom:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *auto_detect.read(),
                                    onchange: move |e| auto_detect.set(e.checked()),
                                }
                                "Auto-detect at startup"
                            }
                            select {
                                id: "hw-accel",
                                class: "select-input",
                                disabled: *auto_detect.read(),
                                value: "{hw_accel}",
                                onchange: move |e| hw_accel.set(e.value()),
                                option { value: "none", "None (Software)" }
                                option { value: "vaapi", "VAAPI (Intel/AMD on Linux)" }
                                option { value: "nvenc", "NVENC (NVIDIA)" }
                                option { value: "qsv", "Quick Sync (Intel)" }
                                option { value: "amf", "AMF (AMD on Windows)" }
                                option { value: "videotoolbox", "VideoToolBox (macOS/Apple)" }
                                option { value: "v4l2m2m", "V4L2M2M (ARM/embedded)" }
                                option { value: "rkmpp", "RKMPP (Rockchip)" }
                            }
                            if *auto_detect.read() {
                                div { class: "field-hint", style: "margin-top:6px",
                                    "Currently using: {hw_accel} (detected at last startup)"
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "encoding-preset", "Encoding Preset" }
                            div { class: "field-hint", "FFmpeg -preset for software transcoding. Faster presets use more CPU; slower presets produce smaller files." }
                            select {
                                id: "encoding-preset",
                                class: "select-input",
                                value: "{encoding_preset}",
                                onchange: move |e| encoding_preset.set(e.value()),
                                option { value: "ultrafast", "Ultrafast (default)" }
                                option { value: "superfast", "Superfast" }
                                option { value: "veryfast", "Veryfast" }
                                option { value: "faster", "Faster" }
                                option { value: "fast", "Fast" }
                                option { value: "medium", "Medium" }
                                option { value: "slow", "Slow" }
                                option { value: "slower", "Slower" }
                                option { value: "slowest", "Slowest" }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "Codec Gates" }
                            div { class: "field-hint", "Allow these codecs for hardware/software encoding." }
                            label { style: "display:flex;align-items:center;gap:8px;margin-bottom:6px",
                                input {
                                    r#type: "checkbox",
                                    checked: *allow_hevc_encoding.read(),
                                    onchange: move |e| allow_hevc_encoding.set(e.checked()),
                                }
                                "Allow HEVC (H.265) encoding"
                            }
                            label { style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *allow_av1_encoding.read(),
                                    onchange: move |e| allow_av1_encoding.set(e.checked()),
                                }
                                "Allow AV1 encoding"
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "Software Encoding Quality (CRF)" }
                            div { class: "field-hint", "Constant Rate Factor for libx264/libx265. Lower = better quality, larger file. Ignored when using hardware encoding or bitrate-limited streams." }
                            div { style: "display:flex;gap:16px;flex-wrap:wrap",
                                div { style: "display:flex;flex-direction:column;gap:4px",
                                    label { r#for: "h264-crf", style: "font-size:0.85em", "H.264 CRF (0–51, default 23)" }
                                    input {
                                        id: "h264-crf",
                                        r#type: "number",
                                        class: "text-input",
                                        style: "width:80px",
                                        min: "0",
                                        max: "51",
                                        value: "{h264_crf}",
                                        onchange: move |e| {
                                            if let Ok(v) = e.value().parse::<u32>() {
                                                h264_crf.set(v.min(51));
                                            }
                                        },
                                    }
                                }
                                div { style: "display:flex;flex-direction:column;gap:4px",
                                    label { r#for: "h265-crf", style: "font-size:0.85em", "H.265 CRF (0–51, default 28)" }
                                    input {
                                        id: "h265-crf",
                                        r#type: "number",
                                        class: "text-input",
                                        style: "width:80px",
                                        min: "0",
                                        max: "51",
                                        value: "{h265_crf}",
                                        onchange: move |e| {
                                            if let Ok(v) = e.value().parse::<u32>() {
                                                h265_crf.set(v.min(51));
                                            }
                                        },
                                    }
                                }
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "HDR Tone Mapping" }
                            div { class: "field-hint", "Convert HDR content to SDR using tone mapping. Without tone mapping, colour metadata is rewritten so clients treat the stream as SDR (may look washed out on some content)." }
                            label { style: "display:flex;align-items:center;gap:8px;margin-bottom:6px",
                                input {
                                    r#type: "checkbox",
                                    checked: *enable_tonemapping.read(),
                                    onchange: move |e| enable_tonemapping.set(e.checked()),
                                }
                                "Software tone mapping (tonemapx, CPU)"
                            }
                            label { style: "display:flex;align-items:center;gap:8px;margin-bottom:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *enable_vpp_tonemapping.read(),
                                    onchange: move |e| enable_vpp_tonemapping.set(e.checked()),
                                }
                                "Hardware VPP tone mapping (tonemap_vaapi, Intel VAAPI/QSV)"
                            }
                            if *enable_tonemapping.read() && !*enable_vpp_tonemapping.read() {
                                div { style: "margin-top:4px",
                                    label { class: "field-label", r#for: "tonemap-algo", style: "font-size:0.85em", "Algorithm" }
                                    select {
                                        id: "tonemap-algo",
                                        class: "select-input",
                                        style: "margin-top:4px",
                                        value: "{tonemapping_algorithm}",
                                        onchange: move |e| tonemapping_algorithm.set(e.value()),
                                        option { value: "hable", "Hable (Filmic, default)" }
                                        option { value: "reinhard", "Reinhard" }
                                        option { value: "mobius", "Mobius" }
                                        option { value: "bt2390", "BT.2390 (perceptual quantizer)" }
                                        option { value: "bt2446a", "BT.2446a" }
                                        option { value: "none", "None (clip)" }
                                    }
                                    div { style: "display:flex;gap:16px;flex-wrap:wrap;margin-top:8px",
                                        div { style: "display:flex;flex-direction:column;gap:4px",
                                            label { r#for: "tonemap-desat", style: "font-size:0.85em", "Desaturation (0 = disabled)" }
                                            input {
                                                id: "tonemap-desat",
                                                r#type: "number",
                                                class: "text-input",
                                                style: "width:80px",
                                                min: "0",
                                                max: "1",
                                                step: "0.1",
                                                value: "{tonemapping_desat}",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<f32>() {
                                                        tonemapping_desat.set(v);
                                                    }
                                                },
                                            }
                                        }
                                        div { style: "display:flex;flex-direction:column;gap:4px",
                                            label { r#for: "tonemap-peak", style: "font-size:0.85em", "Peak luminance nits (0 = auto)" }
                                            input {
                                                id: "tonemap-peak",
                                                r#type: "number",
                                                class: "text-input",
                                                style: "width:90px",
                                                min: "0",
                                                step: "100",
                                                value: "{tonemapping_peak}",
                                                onchange: move |e| {
                                                    if let Ok(v) = e.value().parse::<f32>() {
                                                        tonemapping_peak.set(v);
                                                    }
                                                },
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            ErrorAlert { message: err.clone() }
                        }
                        if *saved.read() {
                            SuccessAlert { message: "Settings saved. Restart the server to apply hardware acceleration changes.".to_string() }
                        }

                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save Settings" }
                            }
                        }
                    }
                }
        }
    }
}

#[component]
pub fn ProbeSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut probe_timeout = use_signal(|| 20_i64);
    let mut probe_timeout_p2p = use_signal(|| 60_i64);
    let mut auto_next_stream = use_signal(|| true);
    let mut max_fallback_streams = use_signal(|| 3_i64);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetSystemConfiguration)
                .await
            {
                Ok(cfg) => {
                    probe_timeout.set(
                        cfg.probe_timeout_secs
                            .unwrap_or(20),
                    );
                    probe_timeout_p2p.set(
                        cfg.probe_timeout_p2p_secs
                            .unwrap_or(60),
                    );
                    auto_next_stream.set(
                        cfg.auto_next_stream_on_probe_fail
                            .unwrap_or(true),
                    );
                    max_fallback_streams.set(
                        cfg.max_probe_fallback_streams
                            .unwrap_or(3),
                    );
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load settings: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state
            .client
            .clone();
        let Some(cfg) = base_cfg
            .peek()
            .clone()
        else {
            return;
        };
        let updated = ServerConfiguration {
            probe_timeout_secs: Some(*probe_timeout.peek()),
            probe_timeout_p2p_secs: Some(*probe_timeout_p2p.peek()),
            auto_next_stream_on_probe_fail: Some(*auto_next_stream.peek()),
            max_probe_fallback_streams: Some(*max_fallback_streams.peek()),
            ..cfg
        };
        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: updated })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        Card { title: "Stream Probing",
            if *loading.read() {
                LoadingText {}
            } else {
                form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                        div { class: "field",
                            label { class: "field-label", r#for: "probe-timeout", "Probe Timeout (seconds)" }
                            div { class: "field-hint",
                                "Seconds to wait for stream probe before giving up on HTTP/local streams."
                            }
                            input {
                                id: "probe-timeout",
                                r#type: "number",
                                class: "text-input",
                                min: "1",
                                max: "300",
                                value: "{probe_timeout}",
                                oninput: move |e| {
                                    if let Ok(v) = e.value().parse::<i64>() {
                                        probe_timeout.set(v);
                                    }
                                },
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "probe-timeout-p2p", "P2P Probe Timeout (seconds)" }
                            div { class: "field-hint",
                                "Seconds to wait for stream probe before giving up on torrent/P2P streams."
                            }
                            input {
                                id: "probe-timeout-p2p",
                                r#type: "number",
                                class: "text-input",
                                min: "1",
                                max: "600",
                                value: "{probe_timeout_p2p}",
                                oninput: move |e| {
                                    if let Ok(v) = e.value().parse::<i64>() {
                                        probe_timeout_p2p.set(v);
                                    }
                                },
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", "Auto Next Stream on Probe Fail" }
                            div { class: "field-hint",
                                "When a stream probe fails, automatically try the next stream with matching resolution and type."
                            }
                            label { style: "display:flex;align-items:center;gap:8px",
                                input {
                                    r#type: "checkbox",
                                    checked: *auto_next_stream.read(),
                                    onchange: move |e| auto_next_stream.set(e.checked()),
                                }
                                "Enabled"
                            }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "max-fallback", "Max Stream Retries" }
                            div { class: "field-hint",
                                "How many alternative streams to try before giving up and returning an error."
                            }
                            input {
                                id: "max-fallback",
                                r#type: "number",
                                class: "text-input",
                                min: "0",
                                max: "20",
                                value: "{max_fallback_streams}",
                                oninput: move |e| {
                                    if let Ok(v) = e.value().parse::<i64>() {
                                        max_fallback_streams.set(v);
                                    }
                                },
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            ErrorAlert { message: err.clone() }
                        }
                        if *saved.read() {
                            SuccessAlert { message: "Settings saved.".to_string() }
                        }

                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save Settings" }
                            }
                        }
                    }
                }
        }
    }
}

#[component]
pub fn SearchSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut movies_remote = use_signal(|| true);
    let mut series_remote = use_signal(|| true);
    let mut tracks_remote = use_signal(|| true);
    let mut albums_remote = use_signal(|| true);
    let mut artists_remote = use_signal(|| true);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetSystemConfiguration)
                .await
            {
                Ok(cfg) => {
                    let enabled = &cfg.search_remote_enabled;
                    let all = enabled.is_none();
                    let list = enabled
                        .as_deref()
                        .unwrap_or(&[]);
                    movies_remote.set(all || list.contains(&"movie".to_string()));
                    series_remote.set(all || list.contains(&"series".to_string()));
                    tracks_remote.set(all || list.contains(&"track".to_string()));
                    albums_remote.set(all || list.contains(&"album".to_string()));
                    artists_remote.set(all || list.contains(&"artist".to_string()));
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load settings: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state
            .client
            .clone();
        let mut cfg = base_cfg
            .peek()
            .clone()
            .unwrap_or_default();
        let mut remote_enabled: Vec<String> = vec!["person".to_string()];
        if *movies_remote.peek() {
            remote_enabled.push("movie".to_string());
        }
        if *series_remote.peek() {
            remote_enabled.push("series".to_string());
        }
        if *tracks_remote.peek() {
            remote_enabled.push("track".to_string());
        }
        if *albums_remote.peek() {
            remote_enabled.push("album".to_string());
        }
        if *artists_remote.peek() {
            remote_enabled.push("artist".to_string());
        }
        cfg.search_remote_enabled = Some(remote_enabled);
        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: cfg })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        Card { title: "Remote Search",
            if *loading.read() {
                LoadingText {}
            } else {
                form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                    div { class: "field",
                        ToggleRow {
                            label: "Movies",
                            checked: *movies_remote.read(),
                            on_change: move |v| movies_remote.set(v),
                        }
                    }
                    div { class: "form-field",
                        ToggleRow {
                            label: "Series",
                            checked: *series_remote.read(),
                            on_change: move |v| series_remote.set(v),
                        }
                    }
                    div { class: "form-field",
                        ToggleRow {
                            label: "Tracks",
                            checked: *tracks_remote.read(),
                            on_change: move |v| tracks_remote.set(v),
                        }
                    }
                    div { class: "form-field",
                        ToggleRow {
                            label: "Albums",
                            checked: *albums_remote.read(),
                            on_change: move |v| albums_remote.set(v),
                        }
                    }
                    div { class: "form-field",
                        ToggleRow {
                            label: "Artists",
                            checked: *artists_remote.read(),
                            on_change: move |v| artists_remote.set(v),
                        }
                    }

                    if let Some(err) = error.read().as_ref() {
                        ErrorAlert { message: err.clone() }
                    }
                    if *saved.read() {
                        SuccessAlert { message: "Settings saved.".to_string() }
                    }

                    div { class: "form-actions",
                        button {
                            r#type: "submit",
                            class: "btn btn-primary",
                            disabled: *saving.read(),
                            if *saving.read() { "Saving…" } else { "Save Settings" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn JellyfinImportCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut jellyfin_url = use_signal(String::new);
    let mut jellyfin_api_key = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut save_error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);
    let mut importing = use_signal(|| false);
    let mut import_error = use_signal(|| Option::<String>::None);
    let mut import_done = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetSystemConfiguration)
                .await
            {
                Ok(cfg) => {
                    jellyfin_url.set(
                        cfg.jellyfin_url
                            .clone()
                            .unwrap_or_default(),
                    );
                    jellyfin_api_key.set(
                        cfg.jellyfin_api_key
                            .clone()
                            .unwrap_or_default(),
                    );
                    base_cfg.set(Some(cfg));
                }
                Err(e) => save_error.set(Some(format!("Failed to load settings: {e}"))),
            }
            loading.set(false);
        });
    });

    let app_state_save = app_state.clone();
    let on_save = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state_save
            .client
            .clone();
        let url = jellyfin_url
            .peek()
            .clone();
        let key = jellyfin_api_key
            .peek()
            .clone();

        let mut cfg = base_cfg
            .peek()
            .clone()
            .unwrap_or_default();
        cfg.jellyfin_url = if url.is_empty() { None } else { Some(url) };
        cfg.jellyfin_api_key = if key.is_empty() { None } else { Some(key) };

        saving.set(true);
        save_error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: cfg })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => save_error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    let on_import = move |_| {
        let client = app_state
            .client
            .clone();
        importing.set(true);
        import_error.set(None);
        import_done.set(false);
        spawn(async move {
            match client
                .execute(StartTask {
                    task_id: "JellyfinImport".into(),
                })
                .await
            {
                Ok(_) => import_done.set(true),
                Err(e) => import_error.set(Some(e.user_message())),
            }
            importing.set(false);
        });
    };

    let url_filled = !jellyfin_url
        .read()
        .is_empty();
    let key_filled = !jellyfin_api_key
        .read()
        .is_empty();
    let can_import = url_filled && key_filled && !*importing.read();

    rsx! {
        Card { title: "Jellyfin Import",
            if *loading.read() {
                LoadingText {}
            } else {
                form {
                    onsubmit: on_save,
                    style: "display:flex;flex-direction:column;gap:14px",

                        div { class: "field",
                            label { class: "field-label", r#for: "jf-url", "Jellyfin URL" }
                            input {
                                id: "jf-url",
                                r#type: "url",
                                class: "field-input",
                                placeholder: "http://192.168.1.x:8096",
                                value: "{jellyfin_url}",
                                oninput: move |e| jellyfin_url.set(e.value()),
                            }
                            p { class: "field-hint", "Base URL of the source Jellyfin server." }
                        }

                        div { class: "field",
                            label { class: "field-label", r#for: "jf-key", "API Key" }
                            input {
                                id: "jf-key",
                                r#type: "password",
                                class: "field-input",
                                placeholder: "••••••••••••••••",
                                value: "{jellyfin_api_key}",
                                oninput: move |e| jellyfin_api_key.set(e.value()),
                            }
                            p { class: "field-hint",
                                "Found in Jellyfin → Dashboard → API Keys."
                            }
                        }

                        if let Some(err) = save_error.read().as_ref() {
                            ErrorAlert { message: err.clone() }
                        }
                        if *saved.read() {
                            SuccessAlert { message: "Settings saved.".to_string() }
                        }

                        div { class: "form-actions", style: "display:flex;gap:8px;align-items:center",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save" }
                            }
                            button {
                                r#type: "button",
                                class: "btn btn-secondary",
                                disabled: !can_import,
                                onclick: on_import,
                                if *importing.read() { "Starting…" } else { "Import Users" }
                            }
                        }

                        if let Some(err) = import_error.read().as_ref() {
                            ErrorAlert { message: err.clone() }
                        }
                        if *import_done.read() {
                            SuccessAlert { message: "Import started. Check the Tasks page for progress.".to_string() }
                        }
                    }
                }
        }
    }
}

#[component]
pub fn P2pSettingsCard(app_state: AppState) -> Element {
    let mut base_cfg: Signal<Option<ServerConfiguration>> = use_signal(|| None);
    let mut p2p_enabled = use_signal(|| true);
    let mut p2p_upload_speed = use_signal(|| 0_i64);
    let mut p2p_download_speed = use_signal(|| 0_i64);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetSystemConfiguration)
                .await
            {
                Ok(cfg) => {
                    p2p_enabled.set(
                        cfg.p2p_enabled
                            .unwrap_or(true),
                    );
                    p2p_upload_speed.set(
                        cfg.p2p_upload_speed_kbps
                            .unwrap_or(0),
                    );
                    p2p_download_speed.set(
                        cfg.p2p_download_speed_kbps
                            .unwrap_or(0),
                    );
                    base_cfg.set(Some(cfg));
                }
                Err(e) => error.set(Some(format!("Failed to load: {e}"))),
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state
            .client
            .clone();
        let Some(cfg) = base_cfg
            .peek()
            .clone()
        else {
            return;
        };
        let updated = ServerConfiguration {
            p2p_enabled: Some(*p2p_enabled.peek()),
            p2p_upload_speed_kbps: Some(*p2p_upload_speed.peek()),
            p2p_download_speed_kbps: Some(*p2p_download_speed.peek()),
            ..cfg
        };
        saving.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            match client
                .execute(UpdateSystemConfiguration { config: updated })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        Card { title: "P2P / Torrent Streams",
            if *loading.read() {
                LoadingText {}
            } else {
                form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                        div { class: "field",
                            label { class: "field-label",
                                input {
                                    r#type: "checkbox",
                                    checked: *p2p_enabled.read(),
                                    oninput: move |e| p2p_enabled.set(e.checked()),
                                }
                                " Enable P2P Streams"
                            }
                            p { class: "field-hint", "Allow torrent/magnet streams from AIO sources." }
                        }

                        if *p2p_enabled.read() {
                            div { class: "field",
                                label { class: "field-label", r#for: "p2p-up", "Upload Speed Limit (KB/s)" }
                                input {
                                    id: "p2p-up",
                                    r#type: "number",
                                    class: "field-input",
                                    min: "0",
                                    value: "{p2p_upload_speed}",
                                    oninput: move |e| {
                                        if let Ok(n) = e.value().parse::<i64>() { p2p_upload_speed.set(n); }
                                    },
                                }
                                p { class: "field-hint", "0 = no uploading (seeding disabled)." }
                            }

                            div { class: "field",
                                label { class: "field-label", r#for: "p2p-down", "Download Speed Limit (KB/s)" }
                                input {
                                    id: "p2p-down",
                                    r#type: "number",
                                    class: "field-input",
                                    min: "0",
                                    value: "{p2p_download_speed}",
                                    oninput: move |e| {
                                        if let Ok(n) = e.value().parse::<i64>() { p2p_download_speed.set(n); }
                                    },
                                }
                                p { class: "field-hint", "0 = unlimited." }
                            }
                        }

                        if let Some(err) = error.read().as_ref() {
                            ErrorAlert { message: err.clone() }
                        }
                        if *saved.read() {
                            SuccessAlert { message: "Settings saved.".to_string() }
                        }
                        div { class: "form-actions",
                            button {
                                r#type: "submit",
                                class: "btn btn-primary",
                                disabled: *saving.read(),
                                if *saving.read() { "Saving…" } else { "Save Settings" }
                            }
                        }
                    }
                }
        }
    }
}

#[component]
pub fn IntroSettingsCard(app_state: AppState) -> Element {
    let mut intro_dir = use_signal(String::new);
    let mut order = use_signal(|| "random".to_string());
    let mut movies = use_signal(|| true);
    let mut season_premieres = use_signal(|| true);
    let mut all_episodes = use_signal(|| false);
    let mut skip_resume = use_signal(|| true);
    let mut loading = use_signal(|| true);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut saved = use_signal(|| false);

    let app_state_load = app_state.clone();
    use_effect(move || {
        let client = app_state_load
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetIntroConfiguration)
                .await
            {
                Ok(opts) => {
                    intro_dir.set(
                        opts.intro_dir
                            .unwrap_or_default(),
                    );
                    order.set(match opts.order {
                        IntroOrder::Sequential => "sequential".to_string(),
                        IntroOrder::Random => "random".to_string(),
                    });
                    movies.set(
                        opts.triggers
                            .movies,
                    );
                    season_premieres.set(
                        opts.triggers
                            .season_premieres,
                    );
                    all_episodes.set(
                        opts.triggers
                            .all_episodes,
                    );
                    skip_resume.set(opts.skip_resume);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load intro settings: {e}")))
                }
            }
            loading.set(false);
        });
    });

    let on_submit = move |e: Event<FormData>| {
        e.prevent_default();
        let client = app_state
            .client
            .clone();
        let dir_val = intro_dir
            .peek()
            .clone();
        let opts = IntroOptions {
            intro_dir: if dir_val
                .trim()
                .is_empty()
            {
                None
            } else {
                Some(dir_val)
            },
            order: if *order.peek() == "sequential" {
                IntroOrder::Sequential
            } else {
                IntroOrder::Random
            },
            triggers: IntroTriggers {
                movies: *movies.peek(),
                season_premieres: *season_premieres.peek(),
                all_episodes: *all_episodes.peek(),
            },
            skip_resume: *skip_resume.peek(),
        };
        saved.set(false);
        error.set(None);
        saving.set(true);
        spawn(async move {
            match client
                .execute(UpdateIntroConfiguration { config: opts })
                .await
            {
                Ok(_) => saved.set(true),
                Err(e) => error.set(Some(e.user_message())),
            }
            saving.set(false);
        });
    };

    rsx! {
        Card { title: "Intro",
            if *loading.read() {
                LoadingText {}
            } else {
                form { onsubmit: on_submit, style: "display:flex;flex-direction:column;gap:14px",
                    div { class: "field",
                        label { class: "field-label", r#for: "intro-dir", "Intro Folder" }
                        div { class: "field-hint",
                            "Absolute path to a folder containing intro video files (mp4, mkv, mov, avi, m4v). Leave blank to disable intros."
                        }
                        input {
                            id: "intro-dir",
                            r#type: "text",
                            class: "text-input",
                            placeholder: "/path/to/intros",
                            value: "{intro_dir}",
                            oninput: move |e| intro_dir.set(e.value()),
                        }
                    }

                    div { class: "field",
                        label { class: "field-label", r#for: "intro-order", "Playback Order" }
                        div { class: "field-hint", "How to pick an intro when multiple files are present." }
                        select {
                            id: "intro-order",
                            class: "select-input",
                            value: "{order}",
                            onchange: move |e| order.set(e.value()),
                            option { value: "random", "Random" }
                            option { value: "sequential", "Sequential (round-robin)" }
                        }
                    }

                    div { class: "field",
                        label { class: "field-label", "Play Before" }
                        div { class: "field-hint", "Which content types trigger an intro." }
                        label { style: "display:flex;align-items:center;gap:8px;margin-bottom:6px",
                            input {
                                r#type: "checkbox",
                                checked: *movies.read(),
                                onchange: move |e| movies.set(e.checked()),
                            }
                            "Movies"
                        }
                        label { style: "display:flex;align-items:center;gap:8px;margin-bottom:6px",
                            input {
                                r#type: "checkbox",
                                checked: *season_premieres.read(),
                                onchange: move |e| season_premieres.set(e.checked()),
                            }
                            "Season premieres (episode 1 of each season)"
                        }
                        label { style: "display:flex;align-items:center;gap:8px",
                            input {
                                r#type: "checkbox",
                                checked: *all_episodes.read(),
                                onchange: move |e| all_episodes.set(e.checked()),
                            }
                            "All episodes"
                        }
                    }

                    div { class: "field",
                        label { class: "field-label", "Resume Behaviour" }
                        label { style: "display:flex;align-items:center;gap:8px",
                            input {
                                r#type: "checkbox",
                                checked: *skip_resume.read(),
                                onchange: move |e| skip_resume.set(e.checked()),
                            }
                            "Skip intro when user is resuming from a saved position"
                        }
                    }

                    if let Some(err) = error.read().as_ref() {
                        ErrorAlert { message: err.clone() }
                    }
                    if *saved.read() {
                        SuccessAlert { message: "Intro settings saved. The intro folder will be scanned in the background.".to_string() }
                    }

                    FormActions {
                        button {
                            r#type: "submit",
                            class: "btn btn-primary",
                            disabled: *saving.read(),
                            if *saving.read() { "Saving…" } else { "Save Settings" }
                        }
                    }
                }
            }
        }
    }
}
