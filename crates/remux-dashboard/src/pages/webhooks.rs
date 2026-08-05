//! Admin page for outgoing webhooks.
//!
//! Two rules govern everything in this module.
//!
//! **A webhook URL is a credential.** Discord's is
//! `https://discord.com/api/webhooks/{id}/{token}` and that token is the whole
//! authentication, so the URL is never written to the browser console (nothing
//! here logs at all) and the list renders only a truncated prefix that stops
//! short of any Discord token.
//!
//! **Every mutation sends a complete [`WebhookDto`].** `WebhookDto` carries no
//! `#[serde(default)]`, so a payload missing one field is a 422 rather than a
//! partial update — even the one-click enable toggle rebuilds the full row.

use crate::{
    components::{Card, EmptyState, ErrorAlert, FormGroup, LoadingText, ToggleRow},
    state::AppState,
};
use dioxus::prelude::*;
use remux_sdks::remux::{
    CreateWebhook, DeleteWebhook, DiscordMentionType, GetUsers, GetWebhooks,
    NotificationType, TestWebhook, UpdateWebhook, UserDto, WebhookDestination,
    WebhookDto, WebhookItemTypes, WebhookKeyValue, WebhookTestResult,
};
use remux_sdks::ClientError;
use std::{collections::HashMap, str::FromStr};
use uuid::Uuid;

/// The Jellyfin webhook plugin's stock `Templates/Discord.handlebars`, verbatim
/// (its UTF-8 BOM stripped).
///
/// This is not decoration. remux follows the plugin exactly: for a Discord
/// destination the operator's template renders the **entire** Discord JSON
/// payload, with the destination's options injected as the variables
/// `MentionType`, `EmbedColor`, `AvatarUrl`, `Username` and `BotUsername`. A
/// Discord webhook with an empty template therefore POSTs an empty body, which
/// is why picking Discord pre-fills this — see [`apply_destination_change`].
const DISCORD_TEMPLATE: &str = r##"{
    "content": "{{MentionType}}",
    "avatar_url": "{{AvatarUrl}}",
    "username": "{{BotUsername}}",
    "embeds": [
        {
            "color": "{{EmbedColor}}",
            "footer": {
                "text": "From {{{ServerName}}}",
                "icon_url": "{{AvatarUrl}}"
            },
            {{#if_equals ItemType 'Season'}}
                "title": "{{{SeriesName}}} {{{Name}}} has been added to {{{ServerName}}}",
            {{else}}
                {{#if_equals ItemType 'Episode'}}
                    "title": "{{{SeriesName}}} S{{SeasonNumber00}}E{{EpisodeNumber00}} {{{Name}}} has been added to {{{ServerName}}}",
                {{else}}
                    "title": "{{{Name}}} ({{Year}}) has been added to {{{ServerName}}}",
                {{/if_equals}}
            {{/if_equals}}
            "thumbnail":{
                "url": "{{ServerUrl}}/Items/{{ItemId}}/Images/Primary"
            },
            "description": "External Links:\n
            {{~#if_exist Provider_imdb~}}
            [IMDb](https://www.imdb.com/title/{{Provider_imdb}}/)\n
            {{~/if_exist~}}
            {{~#if_exist Provider_tmdb~}}
                {{~#if_equals ItemType 'Movie'~}}
                    [TMDb](https://www.themoviedb.org/movie/{{Provider_tmdb}})\n
                {{~else~}}
                    [TMDb](https://www.themoviedb.org/tv/{{Provider_tmdb}})\n
                {{~/if_equals~}}
            {{~/if_exist~}}
            {{~#if_exist Provider_musicbrainzartist~}}
                [MusicBrainz](https://musicbrainz.org/artist/{{Provider_musicbrainzartist}})\n
            {{~/if_exist~}}
            {{~#if_exist Provider_audiodbartist~}}
                [AudioDb](https://theaudiodb.com/artist/{{Provider_audiodbartist}})\n
            {{~/if_exist~}}
            {{~#if_exist Provider_musicbrainztrack~}}
                [MusicBrainz Track](https://musicbrainz.org/track/{{Provider_musicbrainztrack}})\n
            {{~/if_exist~}}
            {{~#if_exist Provider_musicbrainzalbum~}}
                [MusicBrainz Album](https://musicbrainz.org/release/{{Provider_musicbrainzalbum}})\n
            {{~/if_exist~}}
            {{~#if_exist Provider_theaudiodbalbum~}}
                [TADb Album](https://theaudiodb.com/album/{{Provider_theaudiodbalbum}})\n
            {{~/if_exist~}}
            {{~#if_exist Provider_tvmaze~}}
                {{~#if_equals ItemType 'Episode'~}}
                    [TVMaze](https://www.tvmaze.com/episodes/{{Provider_tvmaze}})\n
                {{~/if_equals~}}
                {{~#if_equals ItemType 'Series'~}}
                    [TVMaze](https://www.tvmaze.com/shows/{{Provider_tvmaze}})\n
                {{~/if_equals~}}
            {{~/if_exist~}}
            [Jellyfin]({{ServerUrl}}/web/index.html#!/details?id={{ItemId}}&serverId={{ServerId}})"
        }
    ]
}
"##;

/// The colour the server injects when a Discord hook names none (`0x3399FF`),
/// mirrored here so the field is never blank: the stock template interpolates
/// `EmbedColor` unguarded, and an empty swatch reads as a bug.
///
/// Lower-case on purpose. `<input type="color">` round-trips its value in lower
/// case, and [`WebhookForm::to_dto`] stores the normalized form, so keeping the
/// constant lower-case means the swatch, the text field, the stored value and
/// the placeholder are all one string.
const DEFAULT_EMBED_COLOR: &str = "#3399ff";

/// Every [`NotificationType`], in the order the SDK declares them.
///
/// The list is hand-written, so a variant added to the SDK must be added here
/// too. `every_notification_type_round_trips_through_its_label` keeps the
/// entries themselves honest (each label parses back to the variant, and no
/// entry is duplicated); the array's declared length is what pins the count.
const NOTIFICATION_TYPES: [NotificationType; 15] = [
    NotificationType::ItemAdded,
    NotificationType::ItemDeleted,
    NotificationType::Generic,
    NotificationType::PlaybackStart,
    NotificationType::PlaybackProgress,
    NotificationType::PlaybackStop,
    NotificationType::AuthenticationSuccess,
    NotificationType::AuthenticationFailure,
    NotificationType::SessionStart,
    NotificationType::TaskCompleted,
    NotificationType::UserCreated,
    NotificationType::UserDeleted,
    NotificationType::UserUpdated,
    NotificationType::UserPasswordChanged,
    NotificationType::UserDataSaved,
];

/// Labels for the seven [`WebhookItemTypes`] flags, indexed the same way as
/// [`item_type_flag`] / [`set_item_type_flag`].
const ITEM_TYPE_LABELS: [&str; 7] = [
    "Movies", "Episodes", "Series", "Seasons", "Albums", "Songs", "Videos",
];

const MENTION_TYPES: [DiscordMentionType; 3] = [
    DiscordMentionType::None,
    DiscordMentionType::Here,
    DiscordMentionType::Everyone,
];

/// How much of a webhook URL the list shows. 48 characters stop inside the id
/// segment of a Discord webhook URL — well before the token.
const URL_PREVIEW_LEN: usize = 48;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// A display-only prefix of `url`, char-boundary safe.
fn truncate_url(url: &str, max: usize) -> String {
    if url
        .chars()
        .count()
        <= max
    {
        return url.to_string();
    }
    let head: String = url
        .chars()
        .take(max)
        .collect();
    format!("{head}…")
}

/// `raw` as a `#rrggbb` string an `<input type="color">` accepts, or `None` when
/// it is not a six-digit hex colour. Accepts a leading `#` or not, any case.
fn normalize_hex_color(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let digits = trimmed
        .strip_prefix('#')
        .unwrap_or(trimmed);
    if digits.len() != 6
        || !digits
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    Some(format!("#{}", digits.to_ascii_lowercase()))
}

/// What to feed the colour swatch: the operator's colour when it parses, the
/// server's default otherwise, so the widget is never blank while they type.
fn color_input_value(raw: &str) -> String {
    normalize_hex_color(raw).unwrap_or_else(|| DEFAULT_EMBED_COLOR.to_string())
}

fn item_type_flag(types: &WebhookItemTypes, idx: usize) -> bool {
    match idx {
        0 => types.movies,
        1 => types.episodes,
        2 => types.series,
        3 => types.seasons,
        4 => types.albums,
        5 => types.songs,
        6 => types.videos,
        _ => false,
    }
}

fn set_item_type_flag(types: &mut WebhookItemTypes, idx: usize, value: bool) {
    match idx {
        0 => types.movies = value,
        1 => types.episodes = value,
        2 => types.series = value,
        3 => types.seasons = value,
        4 => types.albums = value,
        5 => types.songs = value,
        6 => types.videos = value,
        _ => {}
    }
}

fn destination_label(destination: &WebhookDestination) -> &'static str {
    match destination {
        WebhookDestination::Generic { .. } => "Generic",
        WebhookDestination::Discord { .. } => "Discord",
    }
}

/// Badge styling for the list, reusing the existing user-badge variants rather
/// than adding CSS: Discord gets the accented one, Generic the muted one.
fn destination_badge_class(destination: &WebhookDestination) -> &'static str {
    match destination {
        WebhookDestination::Generic { .. } => "user-badge user-badge-self",
        WebhookDestination::Discord { .. } => "user-badge user-badge-admin",
    }
}

/// `Some(trimmed)` unless the field is blank — the server treats an empty
/// Discord option and an absent one differently (`if_exist` blocks hinge on it),
/// so a blank input must serialize as `null`, not `""`.
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// `selected` in the canonical order, de-duplicated, so the payload does not
/// depend on the order the operator ticked the boxes.
fn sorted_notification_types(selected: &[NotificationType]) -> Vec<NotificationType> {
    NOTIFICATION_TYPES
        .iter()
        .filter(|t| selected.contains(t))
        .copied()
        .collect()
}

/// The message an operator sees when a mutation fails.
///
/// `ClientError`'s `Display` carries the status, the remux endpoint and the
/// message; `user_message()` is the half that means something to a human, and
/// it is what the rest of the dashboard shows.
fn action_failure(action: &str, error: &ClientError) -> String {
    format!("Failed to {action}: {}", error.user_message())
}

/// One line describing a completed test. A refused delivery is a *result*, not
/// an error: the API call succeeded and returned `success: false`.
fn test_message(result: &WebhookTestResult) -> String {
    if result.success {
        match result.status_code {
            Some(code) => format!("Test delivered — HTTP {code}"),
            None => "Test delivered".to_string(),
        }
    } else {
        let detail = result
            .error
            .clone()
            .unwrap_or_else(|| "delivery failed".to_string());
        match result.status_code {
            Some(code) => format!("Test failed (HTTP {code}) — {detail}"),
            None => format!("Test failed — {detail}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Form state
// ---------------------------------------------------------------------------

/// The editable shape of a webhook.
///
/// Both destinations' options live here at once, so flipping the selector back
/// and forth never discards the headers the operator typed. Which half reaches
/// the wire is decided by `discord` in [`WebhookForm::to_dto`].
///
/// `created_at` / `updated_at` are deliberately absent: they are server-owned
/// (create stamps both, update preserves `created_at` and bumps `updated_at`),
/// so the form sends `null` for them and loses nothing.
#[derive(Clone, PartialEq)]
pub struct WebhookForm {
    /// `None` for a webhook that does not exist yet.
    id: Option<Uuid>,
    name: String,
    enabled: bool,
    url: String,
    template: String,
    discord: bool,
    headers: Vec<WebhookKeyValue>,
    fields: Vec<WebhookKeyValue>,
    avatar_url: String,
    bot_username: String,
    embed_color: String,
    mention_type: DiscordMentionType,
    notification_types: Vec<NotificationType>,
    user_filter: Vec<Uuid>,
    item_types: WebhookItemTypes,
    send_all_properties: bool,
    trim_whitespace: bool,
    skip_empty_message_body: bool,
}

impl Default for WebhookForm {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            enabled: true,
            url: String::new(),
            template: String::new(),
            discord: false,
            headers: Vec::new(),
            fields: Vec::new(),
            avatar_url: String::new(),
            bot_username: String::new(),
            embed_color: DEFAULT_EMBED_COLOR.to_string(),
            mention_type: DiscordMentionType::None,
            notification_types: Vec::new(),
            user_filter: Vec::new(),
            item_types: WebhookItemTypes::default(),
            send_all_properties: false,
            trim_whitespace: false,
            skip_empty_message_body: false,
        }
    }
}

impl WebhookForm {
    fn from_dto(dto: &WebhookDto) -> Self {
        let mut form = Self {
            id: Some(dto.id),
            name: dto
                .name
                .clone(),
            enabled: dto.enabled,
            url: dto
                .url
                .clone(),
            template: dto
                .template
                .clone(),
            notification_types: dto
                .notification_types
                .clone(),
            user_filter: dto
                .user_filter
                .clone(),
            item_types: dto
                .item_types
                .clone(),
            send_all_properties: dto.send_all_properties,
            trim_whitespace: dto.trim_whitespace,
            skip_empty_message_body: dto.skip_empty_message_body,
            ..Self::default()
        };
        match &dto.destination {
            WebhookDestination::Generic { headers, fields } => {
                form.discord = false;
                form.headers = headers.clone();
                form.fields = fields.clone();
            }
            WebhookDestination::Discord {
                avatar_url,
                bot_username,
                embed_color,
                mention_type,
            } => {
                form.discord = true;
                form.avatar_url = avatar_url
                    .clone()
                    .unwrap_or_default();
                form.bot_username = bot_username
                    .clone()
                    .unwrap_or_default();
                // A hook stored without a colour gets the default the server
                // injects anyway, rather than an empty swatch.
                form.embed_color = embed_color
                    .clone()
                    .filter(|c| {
                        !c.trim()
                            .is_empty()
                    })
                    .unwrap_or_else(|| DEFAULT_EMBED_COLOR.to_string());
                form.mention_type = *mention_type;
            }
        }
        form
    }

    /// A **complete** DTO. Never build a partial one: the server's
    /// `WebhookDto` has no field defaults, so an omitted field is a 422.
    fn to_dto(&self) -> WebhookDto {
        let destination = if self.discord {
            WebhookDestination::Discord {
                avatar_url: non_empty(&self.avatar_url),
                bot_username: non_empty(&self.bot_username),
                // Normalized, not passed through: what the swatch shows, what
                // is stored and what reaches Discord must be one value. An
                // unparseable colour is saved as `null`, which makes the server
                // inject the same default the swatch is already displaying.
                embed_color: normalize_hex_color(&self.embed_color),
                mention_type: self.mention_type,
            }
        } else {
            WebhookDestination::Generic {
                headers: self
                    .headers
                    .clone(),
                fields: self
                    .fields
                    .clone(),
            }
        };
        WebhookDto {
            // Ignored by the server on create; it assigns a fresh id.
            id: self
                .id
                .unwrap_or_else(Uuid::nil),
            name: self
                .name
                .trim()
                .to_string(),
            enabled: self.enabled,
            url: self
                .url
                .trim()
                .to_string(),
            template: self
                .template
                .clone(),
            destination,
            notification_types: sorted_notification_types(&self.notification_types),
            user_filter: self
                .user_filter
                .clone(),
            item_types: self
                .item_types
                .clone(),
            send_all_properties: self.send_all_properties,
            trim_whitespace: self.trim_whitespace,
            skip_empty_message_body: self.skip_empty_message_body,
            created_at: None,
            updated_at: None,
        }
    }

    fn is_valid(&self) -> bool {
        !self
            .name
            .trim()
            .is_empty()
            && !self
                .url
                .trim()
                .is_empty()
    }
}

/// Switch the destination, pre-filling the stock Discord template when — and
/// only when — the operator has not written one. Overwriting an edited template
/// would silently destroy their work.
fn apply_destination_change(form: &mut WebhookForm, discord: bool) {
    form.discord = discord;
    if discord
        && form
            .template
            .trim()
            .is_empty()
    {
        form.template = DISCORD_TEMPLATE.to_string();
    }
}

/// `hook` with `enabled` flipped — a whole DTO, not a patch.
fn dto_with_enabled(hook: &WebhookDto, enabled: bool) -> WebhookDto {
    WebhookDto {
        enabled,
        ..hook.clone()
    }
}

/// Outcome of the per-row "Test" button.
#[derive(Clone)]
enum TestState {
    Running,
    /// The API call succeeded. `WebhookTestResult::success` says whether the
    /// *delivery* did.
    Done(WebhookTestResult),
    /// The API call itself failed — transport, auth, or a 4xx/5xx from remux.
    Failed(String),
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

#[component]
pub fn WebhooksPage(app_state: AppState) -> Element {
    let mut hooks: Signal<Vec<WebhookDto>> = use_signal(Vec::new);
    let mut users: Signal<Vec<UserDto>> = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| Option::<String>::None);
    let mut refresh = use_signal(|| 0_u32);

    // Kept apart from `error` on purpose. `error` belongs to the list effect,
    // which clears it on every successful reload and is only rendered when the
    // page is not loading — so a failed toggle or delete written there would be
    // hidden by the reload it triggers and then wiped. This one is owned by the
    // mutation handlers, rendered unconditionally, and never touched by the
    // effect.
    let mut action_error: Signal<Option<String>> = use_signal(|| None);
    let mut editing: Signal<Option<WebhookForm>> = use_signal(|| None);
    let mut to_delete: Signal<Option<(Uuid, String)>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);
    let mut tests: Signal<HashMap<Uuid, TestState>> = use_signal(HashMap::new);

    let app_state_effect = app_state.clone();
    use_effect(move || {
        let _r = *refresh.read();
        loading.set(true);
        let client = app_state_effect
            .client
            .clone();
        spawn(async move {
            match client
                .execute(GetWebhooks)
                .await
            {
                Ok(list) => {
                    hooks.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(action_failure("load webhooks", &e))),
            }
            // A user-filter list we cannot populate is a degraded form, not a
            // page-level failure.
            if let Ok(list) = client
                .execute(GetUsers)
                .await
            {
                users.set(list);
            }
            loading.set(false);
        });
    });

    rsx! {
        Card {
            title: "Webhooks",
            tight: true,
            action: rsx! {
                button {
                    class: "btn btn-primary",
                    style: "height:32px;font-size:.68rem",
                    onclick: move |_| editing.set(Some(WebhookForm::default())),
                    "+ New Webhook"
                }
            },
            p { style: "color:var(--text-muted);font-size:.75rem;padding:0 12px 8px",
                "Webhooks POST a rendered template to an external endpoint whenever a subscribed server event fires."
            }
            // Outside the loading/error chain below: a mutation failure must
            // survive the reload it kicks off.
            if let Some(message) = action_error.read().as_ref() {
                div { style: "padding:0 12px 8px",
                    ErrorAlert { message: message.clone() }
                }
            }
            if *loading.read() {
                LoadingText {}
            } else if let Some(err) = error.read().as_ref() {
                span { class: "loading-text", style: "color:var(--error)", "{err}" }
            } else if hooks.read().is_empty() {
                EmptyState { message: "No webhooks — create one to get started." }
            } else {
                div { class: "data-table-container",
                    div { class: "row-list",
                        for hook in hooks.read().clone() {
                            {
                                let hook_id = hook.id;
                                let name = hook.name.clone();
                                let kind = destination_label(&hook.destination);
                                let kind_class = destination_badge_class(&hook.destination);
                                let url_preview = truncate_url(&hook.url, URL_PREVIEW_LEN);
                                let events = hook.notification_types.len();
                                let enabled = hook.enabled;
                                let hook_toggle = hook.clone();
                                let hook_edit = hook.clone();
                                let client_toggle = app_state.client.clone();
                                let client_test = app_state.client.clone();
                                let delete_name = name.clone();
                                let test_line = tests.read().get(&hook_id).map(|state| match state {
                                    TestState::Running => ("Testing…".to_string(), "var(--text-muted)"),
                                    TestState::Done(result) => (
                                        test_message(result),
                                        if result.success { "var(--success)" } else { "var(--error)" },
                                    ),
                                    TestState::Failed(message) => (message.clone(), "var(--error)"),
                                });
                                rsx! {
                                    div {
                                        class: "flex items-center border-b border-[var(--border)] hover:bg-[rgba(0,0,0,0.03)] even:bg-[rgba(0,0,0,0.02)] even:hover:bg-[rgba(0,0,0,0.03)]",
                                        key: "{hook_id}",
                                        div { class: "flex-1 min-w-0 px-3 py-[10px]",
                                            div { style: "display:flex;align-items:center;gap:8px",
                                                span { style: "font-weight:500;font-size:.85rem", "{name}" }
                                                span { class: "{kind_class}", "{kind}" }
                                            }
                                            div { style: "font-size:.72rem;color:var(--text-muted);font-family:var(--font-mono);margin-top:2px;word-break:break-all",
                                                "{url_preview}"
                                            }
                                            div { style: "font-size:.72rem;color:var(--text-muted);margin-top:2px",
                                                if events == 0 {
                                                    "No event types selected — this webhook never fires"
                                                } else {
                                                    "{events} event types"
                                                }
                                            }
                                            if let Some((message, color)) = test_line {
                                                div { style: "font-size:.72rem;margin-top:4px;color:{color}", "{message}" }
                                            }
                                        }
                                        div { class: "shrink-0 px-3 py-[10px] flex items-center gap-2",
                                            label { class: "toggle", title: if enabled { "Enabled" } else { "Disabled" },
                                                input {
                                                    r#type: "checkbox",
                                                    checked: enabled,
                                                    oninput: move |e| {
                                                        let dto = dto_with_enabled(&hook_toggle, e.checked());
                                                        let c = client_toggle.clone();
                                                        action_error.set(None);
                                                        spawn(async move {
                                                            match c.execute(UpdateWebhook { id: hook_id, webhook: dto }).await {
                                                                Ok(_) => action_error.set(None),
                                                                // The reload below snaps the switch back to
                                                                // the stored value; without this the operator
                                                                // sees a toggle that "won't stick" and no
                                                                // reason why.
                                                                Err(err) => action_error.set(Some(action_failure("update webhook", &err))),
                                                            }
                                                            let v = *refresh.peek() + 1;
                                                            refresh.set(v);
                                                        });
                                                    },
                                                }
                                                span { class: "toggle-track" }
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px",
                                                onclick: move |_| editing.set(Some(WebhookForm::from_dto(&hook_edit))),
                                                "Edit"
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px",
                                                onclick: move |_| {
                                                    tests.write().insert(hook_id, TestState::Running);
                                                    let c = client_test.clone();
                                                    spawn(async move {
                                                        // A refused delivery comes back as Ok(result) with
                                                        // success: false — a result, not an error.
                                                        let outcome = match c.execute(TestWebhook { id: hook_id }).await {
                                                            Ok(result) => TestState::Done(result),
                                                            Err(e) => TestState::Failed(format!(
                                                                "Could not run the test: {}",
                                                                e.user_message()
                                                            )),
                                                        };
                                                        tests.write().insert(hook_id, outcome);
                                                    });
                                                },
                                                "Test"
                                            }
                                            button {
                                                class: "btn btn-ghost",
                                                style: "height:30px;font-size:.68rem;padding:0 10px;color:var(--error);border-color:var(--error)",
                                                onclick: move |_| to_delete.set(Some((hook_id, delete_name.clone()))),
                                                "Delete"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(form) = editing.read().clone() {
            WebhookFormModal {
                app_state: app_state.clone(),
                form,
                users: users.read().clone(),
                on_close: move |_| editing.set(None),
                on_saved: move |_| {
                    editing.set(None);
                    let v = *refresh.peek() + 1;
                    refresh.set(v);
                },
            }
        }

        if let Some((id, name)) = to_delete.read().clone() {
            {
                let client = app_state.client.clone();
                rsx! {
                    div { class: "modal-backdrop",
                        div { class: "modal",
                            div { class: "modal-header",
                                span { class: "modal-title", "Delete Webhook" }
                            }
                            div { class: "modal-body",
                                p { style: "font-size:.85rem",
                                    "Are you sure you want to delete “{name}”? Events will stop being delivered to this endpoint immediately."
                                }
                            }
                            div { class: "modal-footer",
                                button {
                                    class: "btn btn-ghost",
                                    onclick: move |_| to_delete.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "btn btn-ghost",
                                    style: "color:var(--error);border-color:var(--error)",
                                    disabled: *deleting.read(),
                                    onclick: {
                                        let c = client.clone();
                                        move |_| {
                                            deleting.set(true);
                                            action_error.set(None);
                                            let cc = c.clone();
                                            spawn(async move {
                                                match cc.execute(DeleteWebhook { id }).await {
                                                    Ok(_) => action_error.set(None),
                                                    // Without this the row simply stays and the
                                                    // refusal is invisible.
                                                    Err(e) => action_error.set(Some(action_failure("delete webhook", &e))),
                                                }
                                                tests.write().remove(&id);
                                                to_delete.set(None);
                                                deleting.set(false);
                                                let v = *refresh.peek() + 1;
                                                refresh.set(v);
                                            });
                                        }
                                    },
                                    if *deleting.read() { "Deleting…" } else { "Delete" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Create / edit modal
// ---------------------------------------------------------------------------

/// One modal for both create and edit — `form.id` decides which endpoint the
/// save hits.
#[component]
fn WebhookFormModal(
    app_state: AppState,
    form: WebhookForm,
    users: Vec<UserDto>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let mut state = use_signal(|| form.clone());
    let mut saving = use_signal(|| false);
    let mut save_error = use_signal(|| Option::<String>::None);

    // One snapshot per render: reading fields off a clone keeps every handler
    // free to `state.write()` without overlapping the render's borrow.
    let f = state
        .read()
        .clone();
    let is_new =
        f.id.is_none();
    let color_swatch = color_input_value(&f.embed_color);
    // Blank is fine (the server injects its default); anything else that is not
    // a hex colour is silently discarded on save, so say so.
    let color_is_valid = f
        .embed_color
        .trim()
        .is_empty()
        || normalize_hex_color(&f.embed_color).is_some();
    let mention_value = f
        .mention_type
        .to_string();
    let destination_value = if f.discord { "Discord" } else { "Generic" };
    let client = app_state
        .client
        .clone();

    rsx! {
        div { class: "modal-backdrop",
            div { class: "modal modal--wide",
                div { class: "modal-header",
                    span { class: "modal-title", if is_new { "New Webhook" } else { "Edit Webhook" } }
                }
                div { class: "modal-body",
                    FormGroup { label: "Name",
                        input {
                            class: "form-input",
                            r#type: "text",
                            placeholder: "e.g. Discord — new releases",
                            value: "{f.name}",
                            oninput: move |e| state.write().name = e.value(),
                        }
                    }

                    FormGroup { label: "Destination",
                        select {
                            class: "select-input",
                            value: "{destination_value}",
                            onchange: move |e| {
                                let discord = e.value() == "Discord";
                                apply_destination_change(&mut state.write(), discord);
                            },
                            option { value: "Generic", selected: !f.discord, "Generic" }
                            option { value: "Discord", selected: f.discord, "Discord" }
                        }
                    }

                    FormGroup { label: "URL",
                        input {
                            class: "form-input",
                            r#type: "text",
                            placeholder: "https://example.com/hook",
                            value: "{f.url}",
                            oninput: move |e| state.write().url = e.value(),
                        }
                    }

                    ToggleRow {
                        label: "Enabled",
                        checked: f.enabled,
                        on_change: move |v| state.write().enabled = v,
                    }

                    if f.discord {
                        div { class: "form-group",
                            label { class: "form-label", "Discord options" }
                            p { class: "field-hint",
                                "These are injected into the template as MentionType, EmbedColor, AvatarUrl, Username and BotUsername."
                            }
                            FormGroup { label: "Avatar URL",
                                input {
                                    class: "form-input",
                                    r#type: "text",
                                    placeholder: "https://example.com/avatar.png",
                                    value: "{f.avatar_url}",
                                    oninput: move |e| state.write().avatar_url = e.value(),
                                }
                            }
                            FormGroup { label: "Bot username",
                                input {
                                    class: "form-input",
                                    r#type: "text",
                                    placeholder: "Remux",
                                    value: "{f.bot_username}",
                                    oninput: move |e| state.write().bot_username = e.value(),
                                }
                            }
                            FormGroup { label: "Embed color",
                                div { style: "display:flex;gap:8px;align-items:center",
                                    input {
                                        r#type: "color",
                                        style: "width:44px;height:34px;padding:2px;border:1px solid var(--border);border-radius:var(--radius-sm);background:transparent",
                                        value: "{color_swatch}",
                                        oninput: move |e| state.write().embed_color = e.value(),
                                    }
                                    input {
                                        class: "form-input",
                                        r#type: "text",
                                        placeholder: "{DEFAULT_EMBED_COLOR}",
                                        value: "{f.embed_color}",
                                        oninput: move |e| state.write().embed_color = e.value(),
                                    }
                                }
                                if !color_is_valid {
                                    p { class: "field-hint", style: "color:var(--warning)",
                                        "Not a #rrggbb colour — {DEFAULT_EMBED_COLOR} will be used."
                                    }
                                }
                            }
                            FormGroup { label: "Mention type",
                                select {
                                    class: "select-input",
                                    value: "{mention_value}",
                                    onchange: move |e| {
                                        state.write().mention_type =
                                            DiscordMentionType::from_str(&e.value()).unwrap_or_default();
                                    },
                                    for mention in MENTION_TYPES {
                                        {
                                            let label = mention.to_string();
                                            rsx! {
                                                option {
                                                    value: "{label}",
                                                    selected: f.mention_type == mention,
                                                    "{label}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        KeyValueEditor {
                            label: "Headers",
                            hint: "Sent with the request. A pair with an empty key or value is skipped.",
                            key_placeholder: "X-Api-Key",
                            value_placeholder: "secret",
                            items: f.headers.clone(),
                            on_change: move |items| state.write().headers = items,
                        }
                        KeyValueEditor {
                            label: "Template fields",
                            hint: "Extra variables merged into the template data.",
                            key_placeholder: "Environment",
                            value_placeholder: "production",
                            items: f.fields.clone(),
                            on_change: move |items| state.write().fields = items,
                        }
                    }

                    div { class: "form-group",
                        label { class: "form-label", "Template" }
                        p { class: "field-hint",
                            if f.discord {
                                "Handlebars. For Discord the template renders the entire JSON payload — an empty template sends an empty body."
                            } else {
                                "Handlebars. The rendered output is the request body, verbatim."
                            }
                        }
                        textarea {
                            class: "form-input",
                            style: "min-height:200px;resize:vertical;font-family:var(--font-mono);font-size:.76rem;line-height:1.45",
                            spellcheck: false,
                            value: "{f.template}",
                            oninput: move |e| state.write().template = e.value(),
                        }
                    }

                    div { class: "form-group",
                        label { class: "form-label", "Notification types" }
                        if f.notification_types.is_empty() {
                            p { class: "field-hint", style: "color:var(--warning)",
                                "Nothing is selected — this webhook will never fire."
                            }
                        }
                        div { class: "check-row-group",
                            for notification in NOTIFICATION_TYPES {
                                {
                                    let label = notification.to_string();
                                    let checked = f.notification_types.contains(&notification);
                                    rsx! {
                                        label { class: "check-row", key: "{label}",
                                            input {
                                                r#type: "checkbox",
                                                checked,
                                                onchange: move |e| {
                                                    let mut s = state.write();
                                                    if e.checked() {
                                                        if !s.notification_types.contains(&notification) {
                                                            s.notification_types.push(notification);
                                                        }
                                                    } else {
                                                        s.notification_types.retain(|t| *t != notification);
                                                    }
                                                },
                                            }
                                            "{label}"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "form-group",
                        label { class: "form-label", "User filter" }
                        p { class: "field-hint",
                            "Leave everything unchecked to notify for every user."
                        }
                        if users.is_empty() {
                            p { class: "field-hint", "No users available." }
                        } else {
                            div { class: "check-row-group",
                                for user in users.iter().cloned() {
                                    {
                                        let user_id = user.id;
                                        let checked = f.user_filter.contains(&user_id);
                                        rsx! {
                                            label { class: "check-row", key: "{user_id}",
                                                input {
                                                    r#type: "checkbox",
                                                    checked,
                                                    onchange: move |e| {
                                                        let mut s = state.write();
                                                        if e.checked() {
                                                            if !s.user_filter.contains(&user_id) {
                                                                s.user_filter.push(user_id);
                                                            }
                                                        } else {
                                                            s.user_filter.retain(|id| *id != user_id);
                                                        }
                                                    },
                                                }
                                                "{user.name}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "form-group",
                        label { class: "form-label", "Item types" }
                        div { class: "check-row-group",
                            for (idx, label) in ITEM_TYPE_LABELS.iter().enumerate() {
                                {
                                    let checked = item_type_flag(&f.item_types, idx);
                                    rsx! {
                                        label { class: "check-row", key: "{label}",
                                            input {
                                                r#type: "checkbox",
                                                checked,
                                                onchange: move |e| {
                                                    let mut s = state.write();
                                                    set_item_type_flag(&mut s.item_types, idx, e.checked());
                                                },
                                            }
                                            "{label}"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "form-group",
                        label { class: "form-label", "Options" }
                        ToggleRow {
                            label: "Send all properties",
                            checked: f.send_all_properties,
                            on_change: move |v| state.write().send_all_properties = v,
                        }
                        ToggleRow {
                            label: "Trim whitespace",
                            checked: f.trim_whitespace,
                            on_change: move |v| state.write().trim_whitespace = v,
                        }
                        ToggleRow {
                            label: "Skip empty message body",
                            checked: f.skip_empty_message_body,
                            on_change: move |v| state.write().skip_empty_message_body = v,
                        }
                    }

                    if let Some(err) = save_error.read().as_ref() {
                        ErrorAlert { message: err.clone() }
                    }
                }
                div { class: "modal-footer",
                    button {
                        class: "btn btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        class: "btn btn-primary",
                        disabled: *saving.read() || !f.is_valid(),
                        onclick: move |_| {
                            let snapshot = state.peek().clone();
                            if !snapshot.is_valid() {
                                return;
                            }
                            let dto = snapshot.to_dto();
                            let id = snapshot.id;
                            saving.set(true);
                            save_error.set(None);
                            let c = client.clone();
                            spawn(async move {
                                let outcome = match id {
                                    Some(id) => c.execute(UpdateWebhook { id, webhook: dto }).await.map(|_| ()),
                                    None => c.execute(CreateWebhook { webhook: dto }).await.map(|_| ()),
                                };
                                match outcome {
                                    Ok(()) => on_saved.call(()),
                                    Err(e) => {
                                        save_error.set(Some(action_failure("save webhook", &e)))
                                    }
                                }
                                saving.set(false);
                            });
                        },
                        if *saving.read() { "Saving…" } else { "Save" }
                    }
                }
            }
        }
    }
}

/// Dynamic list of key/value pairs. Stateless — the parent owns the vector and
/// receives a whole new one on every edit.
#[component]
fn KeyValueEditor(
    label: String,
    hint: String,
    key_placeholder: String,
    value_placeholder: String,
    items: Vec<WebhookKeyValue>,
    on_change: EventHandler<Vec<WebhookKeyValue>>,
) -> Element {
    rsx! {
        div { class: "form-group",
            label { class: "form-label", "{label}" }
            p { class: "field-hint", "{hint}" }
            for (idx, pair) in items.iter().enumerate() {
                {
                    let on_key = items.clone();
                    let on_value = items.clone();
                    let on_remove = items.clone();
                    rsx! {
                        div {
                            key: "{idx}",
                            style: "display:flex;gap:6px;align-items:center;margin-bottom:6px",
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: "{key_placeholder}",
                                value: "{pair.key}",
                                oninput: move |e| {
                                    let mut next = on_key.clone();
                                    next[idx].key = e.value();
                                    on_change.call(next);
                                },
                            }
                            input {
                                class: "form-input",
                                r#type: "text",
                                placeholder: "{value_placeholder}",
                                value: "{pair.value}",
                                oninput: move |e| {
                                    let mut next = on_value.clone();
                                    next[idx].value = e.value();
                                    on_change.call(next);
                                },
                            }
                            button {
                                class: "btn btn-ghost",
                                style: "height:36px;flex-shrink:0;color:var(--error);border-color:var(--error)",
                                onclick: move |_| {
                                    let mut next = on_remove.clone();
                                    next.remove(idx);
                                    on_change.call(next);
                                },
                                "×"
                            }
                        }
                    }
                }
            }
            {
                let on_add = items.clone();
                rsx! {
                    button {
                        class: "btn btn-ghost",
                        style: "height:30px;font-size:.68rem;padding:0 10px",
                        onclick: move |_| {
                            let mut next = on_add.clone();
                            next.push(WebhookKeyValue::default());
                            on_change.call(next);
                        },
                        "+ Add"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use remux_sdks::EnumCount;

    fn kv(key: &str, value: &str) -> WebhookKeyValue {
        WebhookKeyValue {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    fn discord_dto() -> WebhookDto {
        WebhookDto {
            id: Uuid::from_u128(0xdead_beef),
            name: "Releases".to_string(),
            enabled: true,
            url: "https://discord.com/api/webhooks/1234/token".to_string(),
            template: "{\"content\":\"{{Name}}\"}".to_string(),
            destination: WebhookDestination::Discord {
                avatar_url: Some("https://example.com/a.png".to_string()),
                bot_username: Some("Remux".to_string()),
                embed_color: Some("#aa5cc3".to_string()),
                mention_type: DiscordMentionType::Everyone,
            },
            notification_types: vec![
                NotificationType::ItemAdded,
                NotificationType::PlaybackStop,
            ],
            user_filter: vec![Uuid::from_u128(7), Uuid::from_u128(9)],
            item_types: WebhookItemTypes {
                movies: true,
                episodes: false,
                series: true,
                seasons: false,
                albums: true,
                songs: false,
                videos: true,
            },
            send_all_properties: true,
            trim_whitespace: true,
            skip_empty_message_body: true,
            created_at: None,
            updated_at: None,
        }
    }

    fn generic_dto() -> WebhookDto {
        WebhookDto {
            destination: WebhookDestination::Generic {
                headers: vec![kv("X-Api-Key", "s3cret"), kv("X-Other", "v")],
                fields: vec![kv("Environment", "production")],
            },
            ..discord_dto()
        }
    }

    // -- url preview --------------------------------------------------------

    #[test]
    fn a_short_url_is_shown_whole() {
        assert_eq!(
            truncate_url("https://example.com/hook", 48),
            "https://example.com/hook"
        );
    }

    #[test]
    fn a_long_url_is_cut_before_a_discord_token() {
        let url = "https://discord.com/api/webhooks/123456789012345678/AbCdEfGhIjKlMnOpQrStUvWxYz";
        let shown = truncate_url(url, URL_PREVIEW_LEN);
        assert!(
            shown.ends_with('…'),
            "a long url must be visibly truncated: {shown}"
        );
        assert!(
            !shown.contains("AbCdEf"),
            "the token must never reach the DOM: {shown}"
        );
    }

    #[test]
    fn truncation_does_not_split_a_char() {
        // Every char is 3 bytes; a byte-wise cut would panic.
        let url = "https://example.com/日本語日本語日本語";
        assert_eq!(
            truncate_url(url, 22)
                .chars()
                .count(),
            23
        );
    }

    // -- colour -------------------------------------------------------------

    #[test]
    fn a_six_digit_hex_is_normalised_for_the_swatch() {
        assert_eq!(normalize_hex_color("#AA5CC3"), Some("#aa5cc3".to_string()));
        assert_eq!(normalize_hex_color("aa5cc3"), Some("#aa5cc3".to_string()));
        assert_eq!(
            normalize_hex_color("  #000000 "),
            Some("#000000".to_string())
        );
    }

    #[test]
    fn anything_that_is_not_a_six_digit_hex_is_rejected() {
        for raw in ["", "#", "#12345", "#1234567", "#GGGGGG", "red"] {
            assert_eq!(normalize_hex_color(raw), None, "raw = {raw:?}");
        }
    }

    #[test]
    fn the_swatch_falls_back_to_the_server_default() {
        assert_eq!(color_input_value(""), "#3399ff");
        assert_eq!(color_input_value("#1a2b3c"), "#1a2b3c");
    }

    // -- item types ---------------------------------------------------------

    #[test]
    fn every_item_type_index_round_trips() {
        for idx in 0..ITEM_TYPE_LABELS.len() {
            let mut types = WebhookItemTypes::default();
            set_item_type_flag(&mut types, idx, false);
            assert!(!item_type_flag(&types, idx), "idx {idx} did not clear");
            set_item_type_flag(&mut types, idx, true);
            assert!(item_type_flag(&types, idx), "idx {idx} did not set");
            // Clearing one flag must not disturb the others.
            let mut only = WebhookItemTypes::default();
            set_item_type_flag(&mut only, idx, false);
            let cleared = (0..ITEM_TYPE_LABELS.len())
                .filter(|i| !item_type_flag(&only, *i))
                .count();
            assert_eq!(cleared, 1, "idx {idx} cleared more than itself");
        }
    }

    // -- notification types -------------------------------------------------

    #[test]
    fn every_notification_type_round_trips_through_its_label() {
        let mut labels: Vec<String> = NOTIFICATION_TYPES
            .iter()
            .map(|t| t.to_string())
            .collect();
        for (label, expected) in labels
            .iter()
            .zip(NOTIFICATION_TYPES.iter())
        {
            assert_eq!(
                NotificationType::from_str(label).ok(),
                Some(*expected),
                "label {label} did not parse back"
            );
        }
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 15, "the list must have no duplicates");
    }

    /// The form's list is hand-written; this is what stops a variant added to
    /// the SDK from silently going missing from the checkbox grid.
    #[test]
    fn the_list_covers_every_variant_the_sdk_declares() {
        assert_eq!(
            NOTIFICATION_TYPES.len(),
            NotificationType::COUNT,
            "a NotificationType variant is missing from NOTIFICATION_TYPES"
        );
    }

    #[test]
    fn selection_is_sorted_into_the_canonical_order_and_deduped() {
        let selected = vec![
            NotificationType::UserDeleted,
            NotificationType::ItemAdded,
            NotificationType::UserDeleted,
        ];
        assert_eq!(
            sorted_notification_types(&selected),
            vec![NotificationType::ItemAdded, NotificationType::UserDeleted]
        );
        assert!(sorted_notification_types(&[]).is_empty());
    }

    // -- form round-trip ----------------------------------------------------

    /// The form must not silently drop a field: a fully populated hook that
    /// goes through the form and back is the same JSON the server sent.
    #[test]
    fn a_discord_webhook_round_trips_without_losing_a_field() {
        let original = discord_dto();
        let round = WebhookForm::from_dto(&original).to_dto();
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&round).unwrap()
        );
    }

    #[test]
    fn a_generic_webhook_round_trips_without_losing_a_field() {
        let original = generic_dto();
        let round = WebhookForm::from_dto(&original).to_dto();
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&round).unwrap()
        );
    }

    #[test]
    fn the_other_destinations_options_survive_a_round_trip_through_the_selector() {
        let mut form = WebhookForm::from_dto(&generic_dto());
        apply_destination_change(&mut form, true);
        apply_destination_change(&mut form, false);
        let dto = form.to_dto();
        match dto.destination {
            WebhookDestination::Generic { headers, fields } => {
                assert_eq!(headers.len(), 2, "headers were dropped");
                assert_eq!(fields.len(), 1, "fields were dropped");
            }
            other => panic!("expected Generic, got {other:?}"),
        }
    }

    #[test]
    fn a_discord_hook_stored_without_a_colour_gets_the_default() {
        let dto = WebhookDto {
            destination: WebhookDestination::Discord {
                avatar_url: None,
                bot_username: None,
                embed_color: None,
                mention_type: DiscordMentionType::None,
            },
            ..discord_dto()
        };
        let round = WebhookForm::from_dto(&dto).to_dto();
        match round.destination {
            WebhookDestination::Discord {
                embed_color,
                avatar_url,
                bot_username,
                ..
            } => {
                assert_eq!(embed_color.as_deref(), Some(DEFAULT_EMBED_COLOR));
                // Blank identity options must stay absent — the stock template
                // guards them with `if_exist`, which an empty string flips.
                assert_eq!(avatar_url, None);
                assert_eq!(bot_username, None);
            }
            other => panic!("expected Discord, got {other:?}"),
        }
    }

    /// What the swatch shows, what is saved and what Discord receives must be
    /// one value: an unparseable colour is dropped on save so the server's
    /// default — the colour the swatch is already displaying — applies.
    #[test]
    fn an_unparseable_colour_is_not_sent_verbatim() {
        let form = WebhookForm {
            discord: true,
            embed_color: "purple".to_string(),
            ..WebhookForm::default()
        };
        assert_eq!(color_input_value(&form.embed_color), DEFAULT_EMBED_COLOR);
        match form
            .to_dto()
            .destination
        {
            WebhookDestination::Discord { embed_color, .. } => assert_eq!(
                embed_color, None,
                "an invalid colour must not reach the server"
            ),
            other => panic!("expected Discord, got {other:?}"),
        }
    }

    #[test]
    fn a_colour_is_stored_in_the_form_the_swatch_uses() {
        let form = WebhookForm {
            discord: true,
            embed_color: "#AA5CC3".to_string(),
            ..WebhookForm::default()
        };
        match form
            .to_dto()
            .destination
        {
            WebhookDestination::Discord { embed_color, .. } => {
                assert_eq!(embed_color.as_deref(), Some("#aa5cc3"));
            }
            other => panic!("expected Discord, got {other:?}"),
        }
    }

    #[test]
    fn a_new_webhook_carries_a_nil_id_and_no_timestamps() {
        let dto = WebhookForm::default().to_dto();
        assert_eq!(dto.id, Uuid::nil());
        assert_eq!(dto.created_at, None);
        assert_eq!(dto.updated_at, None);
        assert!(dto.enabled);
    }

    #[test]
    fn the_enable_toggle_sends_a_complete_dto() {
        let hook = discord_dto();
        let flipped = dto_with_enabled(&hook, false);
        assert!(!flipped.enabled);
        let expected = serde_json::to_value(WebhookDto {
            enabled: false,
            ..hook
        })
        .unwrap();
        assert_eq!(serde_json::to_value(&flipped).unwrap(), expected);
    }

    #[test]
    fn a_webhook_needs_a_name_and_a_url_before_it_can_be_saved() {
        let mut form = WebhookForm::default();
        assert!(!form.is_valid());
        form.name = "  ".to_string();
        form.url = "https://example.com".to_string();
        assert!(!form.is_valid(), "whitespace is not a name");
        form.name = "Hook".to_string();
        assert!(form.is_valid());
    }

    // -- destination switch -------------------------------------------------

    #[test]
    fn picking_discord_prefills_the_stock_template() {
        let mut form = WebhookForm::default();
        apply_destination_change(&mut form, true);
        assert!(form.discord);
        assert_eq!(form.template, DISCORD_TEMPLATE);
        assert!(
            form.template
                .contains("{{MentionType}}"),
            "the stock template must expose the injected variables"
        );
    }

    #[test]
    fn picking_discord_never_overwrites_an_edited_template() {
        let mut form = WebhookForm {
            template: "mine".to_string(),
            ..WebhookForm::default()
        };
        apply_destination_change(&mut form, true);
        assert_eq!(form.template, "mine");
    }

    #[test]
    fn going_back_to_generic_leaves_the_template_alone() {
        let mut form = WebhookForm::default();
        apply_destination_change(&mut form, true);
        apply_destination_change(&mut form, false);
        assert!(!form.discord);
        assert_eq!(form.template, DISCORD_TEMPLATE);
    }

    // -- test result rendering ----------------------------------------------

    #[test]
    fn a_delivered_test_reads_as_a_success() {
        let message = test_message(&WebhookTestResult {
            success: true,
            status_code: Some(204),
            error: None,
        });
        assert_eq!(message, "Test delivered — HTTP 204");
    }

    /// The endpoint answers `200 OK` with `success: false` when the *target*
    /// refuses. That is a result, not an API error, and must render as one.
    #[test]
    fn a_refused_delivery_reads_as_a_failed_result() {
        let message = test_message(&WebhookTestResult {
            success: false,
            status_code: Some(401),
            error: Some("endpoint returned 401 Unauthorized".to_string()),
        });
        assert!(message.starts_with("Test failed (HTTP 401)"), "{message}");
        assert!(message.contains("401 Unauthorized"), "{message}");
    }

    // -- mutation failures --------------------------------------------------

    /// A failed toggle, save or delete must reach the operator as the server's
    /// sentence, not as the SDK's `Display` with its status and endpoint noise.
    #[test]
    fn a_failed_mutation_shows_the_servers_message_only() {
        let error = ClientError::Http {
            status: 400,
            message: "webhook url must use http or https".to_string(),
            endpoint: Some("/remux/webhooks".to_string()),
            body: Some(
                "{\"title\":\"webhook url must use http or https\"}".to_string(),
            ),
        };
        let shown = action_failure("save webhook", &error);
        assert_eq!(
            shown,
            "Failed to save webhook: webhook url must use http or https"
        );
        assert!(!shown.contains("status="), "{shown}");
        assert!(!shown.contains("endpoint="), "{shown}");
    }

    #[test]
    fn a_transport_failure_reads_without_a_status() {
        let message = test_message(&WebhookTestResult {
            success: false,
            status_code: None,
            error: Some("connection refused".to_string()),
        });
        assert_eq!(message, "Test failed — connection refused");
    }

    // -- misc ---------------------------------------------------------------

    #[test]
    fn a_blank_option_serialises_as_absent_not_empty() {
        assert_eq!(non_empty("  "), None);
        assert_eq!(non_empty(" x "), Some("x".to_string()));
    }

    #[test]
    fn destinations_are_labelled_for_the_list_badge() {
        assert_eq!(
            destination_label(&WebhookDestination::Generic {
                headers: vec![],
                fields: vec![]
            }),
            "Generic"
        );
        assert_eq!(
            destination_label(&WebhookDestination::Discord {
                avatar_url: None,
                bot_username: None,
                embed_color: None,
                mention_type: DiscordMentionType::None,
            }),
            "Discord"
        );
    }

    #[test]
    fn destination_badges_reuse_an_existing_style() {
        let generic = WebhookDestination::Generic {
            headers: vec![],
            fields: vec![],
        };
        let discord = WebhookDestination::Discord {
            avatar_url: None,
            bot_username: None,
            embed_color: None,
            mention_type: DiscordMentionType::None,
        };
        assert!(
            destination_badge_class(&generic).starts_with("user-badge "),
            "the badge must carry the base class"
        );
        assert_ne!(
            destination_badge_class(&generic),
            destination_badge_class(&discord),
            "the two destinations must be visually distinguishable"
        );
    }
}
