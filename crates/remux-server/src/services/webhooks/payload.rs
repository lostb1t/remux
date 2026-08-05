//! The variable dictionary handed to webhook templates.
//!
//! Key names are the ones the Jellyfin webhook plugin uses, verbatim: they are
//! the public surface every operator template is written against, so a renamed
//! key is a silent breaking change.
//!
//! [`build_data`] is deliberately pure and synchronous. Everything it needs
//! that lives in the database is resolved beforehand by [`enrich_item`] and
//! [`ServerInfo::load`], both of which run once per event (resp. once per
//! process) rather than once per webhook.

use super::events::{DeviceEventData, PlaybackEventData, UserEventData, WebhookEvent};
use crate::{AppContext, db};
use remux_sdks::remux::{
    DiscordMentionType, MediaStream, MediaStreamType, WebhookDestination,
};
use serde_json::{Map, Value};
use std::borrow::Cow;
use tracing::warn;
use uuid::Uuid;

/// Jellyfin ticks in one second (a tick is 100 ns).
const TICKS_PER_SECOND: i64 = 10_000_000;

/// Fraction of the runtime past which playback counts as completed.
const COMPLETION_RATIO: f64 = 0.9;

/// Name used when the server has none configured.
const DEFAULT_SERVER_NAME: &str = "remux";

/// Discord's own default embed colour (`0x3399FF`), as hardcoded by the
/// Jellyfin webhook plugin's stock Discord templates. Used when a hook names no
/// colour or names an unparseable one.
const DEFAULT_EMBED_COLOR: u32 = 3_381_759;

/// The `ItemType` a template sees. Narrower than [`db::MediaKind`] on purpose:
/// it is the Jellyfin `BaseItemKind` subset the webhook plugin emits, and
/// everything that is not one of the named kinds is reported as `Video`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display)]
pub(crate) enum ItemType {
    Movie,
    Episode,
    Series,
    Season,
    MusicAlbum,
    Audio,
    Video,
}

impl From<&db::MediaKind> for ItemType {
    fn from(kind: &db::MediaKind) -> Self {
        match kind {
            db::MediaKind::Movie => Self::Movie,
            db::MediaKind::Episode => Self::Episode,
            db::MediaKind::Series => Self::Series,
            db::MediaKind::Season => Self::Season,
            db::MediaKind::Album => Self::MusicAlbum,
            db::MediaKind::Track => Self::Audio,
            _ => Self::Video,
        }
    }
}

/// The library item an event is about, resolved once per event.
pub(crate) struct ItemContext {
    pub media: db::Media,
    /// Season (episode) or album (track).
    pub parent: Option<db::Media>,
    /// Series (episode) or artist (track).
    pub grandparent: Option<db::Media>,
    pub genres: Vec<String>,
}

/// The identity of this server, resolved once when the dispatcher starts.
#[derive(Debug, Clone)]
pub(crate) struct ServerInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    /// `Config::public_url`, or empty when the operator has not set one.
    pub url: String,
}

impl ServerInfo {
    pub(crate) async fn load(ctx: &AppContext) -> Self {
        let config = db::Settings::get_config_or_default(&ctx.db).await;
        let name = config
            .server_name
            .filter(|name| {
                !name
                    .trim()
                    .is_empty()
            })
            .unwrap_or_else(|| DEFAULT_SERVER_NAME.to_string());
        Self {
            id: crate::common::server_id(),
            name,
            version: env!("CARGO_PKG_VERSION").to_string(),
            url: ctx
                .config
                .public_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .unwrap_or_default()
                .to_string(),
        }
    }
}

/// Resolve the item an event is about, plus its parents and genres.
///
/// `ItemDeleted` carries the row captured before the DELETE, so it never needs
/// the item lookup — the row is already gone.
pub(crate) async fn enrich_item(
    ctx: &AppContext,
    event: &WebhookEvent,
) -> Option<ItemContext> {
    let media = match event {
        WebhookEvent::ItemDeleted { item } => (**item).clone(),
        other => load_media(ctx, other.item_id()?).await?,
    };

    let parent = load_optional(ctx, media.parent_id).await;
    let grandparent = load_optional(ctx, media.grandparent_id).await;
    let genres = match db::Media::genre_names(&ctx.db, &media.id).await {
        Ok(genres) => genres,
        Err(e) => {
            warn!(item = %media.id, error = %e, "failed to load webhook item genres");
            Vec::new()
        }
    };

    Some(ItemContext {
        media,
        parent,
        grandparent,
        genres,
    })
}

async fn load_media(ctx: &AppContext, id: Uuid) -> Option<db::Media> {
    match db::Media::get_by_id(&ctx.db, &id).await {
        Ok(media) => media,
        Err(e) => {
            warn!(item = %id, error = %e, "failed to load webhook item");
            None
        }
    }
}

async fn load_optional(ctx: &AppContext, id: Option<Uuid>) -> Option<db::Media> {
    load_media(ctx, id?).await
}

/// The variables shared by every webhook this event reaches.
pub(crate) fn build_data(
    server: &ServerInfo,
    event: &WebhookEvent,
    item: Option<&ItemContext>,
) -> Map<String, Value> {
    let mut data = Map::new();

    put(&mut data, "ServerId", &server.id);
    put(&mut data, "ServerName", &server.name);
    put(&mut data, "ServerVersion", &server.version);
    put(&mut data, "ServerUrl", &server.url);
    put(
        &mut data,
        "NotificationType",
        event
            .notification_type()
            .to_string(),
    );
    put(&mut data, "Timestamp", chrono::Local::now().to_rfc3339());
    put(&mut data, "UtcTimestamp", chrono::Utc::now().to_rfc3339());

    if let Some(item) = item {
        put_item(&mut data, item);
    }
    put_event(&mut data, event, item);

    data
}

/// Per-hook overlay: the destination's own settings become template variables,
/// exactly as the Jellyfin webhook plugin's clients do before rendering.
///
/// - `Generic` contributes the operator-defined `fields` under their own keys
///   (`GenericClient.SendAsync`).
/// - `Discord` contributes `MentionType`, `EmbedColor`, `AvatarUrl`, `Username`
///   and `BotUsername` (`DiscordClient.SendAsync`) — which is what lets a
///   Discord template copied from the plugin render the whole payload itself.
///   `EmbedColor` is the one intended deviation: always present, see below.
///
/// Borrowed — and therefore free — when the hook contributes nothing.
pub(crate) fn with_hook_fields<'a>(
    data: &'a Map<String, Value>,
    hook: &db::Webhook,
) -> Cow<'a, Map<String, Value>> {
    match &hook.destination {
        WebhookDestination::Generic { fields, .. } => {
            if fields.is_empty() {
                return Cow::Borrowed(data);
            }
            let mut merged = data.clone();
            for field in fields {
                // `GenericClient.SendAsync` skips a pair when either half is
                // empty — the same rule the headers half of that method applies.
                let (Some(key), Some(value)) = (
                    non_empty(Some(
                        field
                            .key
                            .as_str(),
                    )),
                    non_empty(Some(
                        field
                            .value
                            .as_str(),
                    )),
                ) else {
                    continue;
                };
                merged.insert(key.to_string(), Value::String(value.to_string()));
            }
            Cow::Owned(merged)
        }
        // Key spellings, value formats and presence rules follow
        // `DiscordClient.SendAsync` literally: `MentionType` is always set (to
        // the empty string for `None`) and a username lands under both
        // `Username` and `BotUsername`, present only when configured.
        WebhookDestination::Discord {
            avatar_url,
            bot_username,
            embed_color,
            mention_type,
        } => {
            let mut merged = data.clone();
            merged.insert(
                "MentionType".into(),
                Value::String(mention_type_variable(*mention_type).to_string()),
            );
            // Intended deviation: the plugin omits `EmbedColor` when the hook
            // names no colour, which makes its own stock `Discord.handlebars`
            // render `"color": ""` and Discord reject the payload with a 400.
            // The key is therefore always present, defaulted. This costs no
            // template-behaviour parity: across all five stock Discord
            // templates `{{EmbedColor}}` appears exactly once, as a bare
            // interpolation, never guarded by `if_exist` — the four per-event
            // templates hardcode a literal colour and ignore the variable.
            merged.insert(
                "EmbedColor".into(),
                Value::Number(
                    non_empty(embed_color.as_deref())
                        .map_or(DEFAULT_EMBED_COLOR, parse_embed_color)
                        .into(),
                ),
            );
            // `AvatarUrl` / `Username` / `BotUsername` keep strict presence
            // parity: the stock templates *do* guard these with `if_exist`, so
            // an always-present empty string would flip those blocks.
            if let Some(url) = non_empty(avatar_url.as_deref()) {
                merged.insert("AvatarUrl".into(), Value::String(url.to_string()));
            }
            if let Some(username) = non_empty(bot_username.as_deref()) {
                merged.insert("Username".into(), Value::String(username.to_string()));
                merged
                    .insert("BotUsername".into(), Value::String(username.to_string()));
            }
            Cow::Owned(merged)
        }
    }
}

/// What `{{MentionType}}` renders to. Empty for `None`, as in the plugin's
/// `DiscordClient.GetMentionType`.
fn mention_type_variable(mention_type: DiscordMentionType) -> &'static str {
    match mention_type {
        DiscordMentionType::None => "",
        DiscordMentionType::Here => "@here",
        DiscordMentionType::Everyone => "@everyone",
    }
}

/// `#RRGGBB` (or bare `RRGGBB`) as the integer Discord wants, mirroring the
/// plugin's `FormatColorCode` — except that the plugin slices `hexCode[1..6]`
/// and silently drops the last hex digit, turning `#AA5CC3` into 697 804. That
/// bug is deliberately **not** reproduced: an admin gets the colour they pick.
///
/// Anything unparseable falls back to [`DEFAULT_EMBED_COLOR`] rather than
/// throwing as the plugin does — this is operator input and must never fail a
/// delivery.
pub(crate) fn parse_embed_color(hex: &str) -> u32 {
    let hex = hex
        .trim()
        .trim_start_matches('#');
    if hex.len() != 6
        || !hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return DEFAULT_EMBED_COLOR;
    }
    u32::from_str_radix(hex, 16).unwrap_or(DEFAULT_EMBED_COLOR)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

// --- item -----------------------------------------------------------------

fn put_item(data: &mut Map<String, Value>, item: &ItemContext) {
    let media = &item.media;

    put(data, "Name", &media.title);
    if let Some(overview) = media
        .description
        .as_deref()
    {
        put(data, "Overview", overview);
    }
    put(data, "ItemId", simple_id(&media.id));
    put(data, "ItemType", ItemType::from(&media.kind).to_string());

    // Always emitted, zeroed when unknown: imported templates print them
    // unconditionally, so an absent key would render as an empty string.
    let runtime = media
        .runtime
        .unwrap_or(0);
    data.insert("RunTimeTicks".to_string(), Value::from(ticks(runtime)));
    put(data, "RunTime", hms(runtime));

    put_year(data, item);
    if let Some(released_at) = media.released_at {
        put(
            data,
            "PremiereDate",
            released_at
                .date()
                .format("%Y-%m-%d")
                .to_string(),
        );
    }

    if !item
        .genres
        .is_empty()
    {
        put(
            data,
            "Genres",
            item.genres
                .join(", "),
        );
    }

    match media.kind {
        db::MediaKind::Episode => put_episode(data, item),
        db::MediaKind::Season => put_season(data, item),
        db::MediaKind::Track => {
            if let Some(album) = item
                .parent
                .as_ref()
            {
                put(data, "Album", &album.title);
            }
            if let Some(artist) = item
                .grandparent
                .as_ref()
            {
                put(data, "Artist", &artist.title);
            }
        }
        _ => {}
    }

    put_providers(data, &media.external_ids);
    if let Some(probe) = media
        .probe_data
        .as_ref()
    {
        put_streams(data, &probe.media_streams);
    }
}

/// `Year` for an episode or a season is the *series'* production year, as the
/// plugin reads it off `Series.ProductionYear`. Everything else reports its own
/// release year.
fn put_year(data: &mut Map<String, Value>, item: &ItemContext) {
    let released_at = match item
        .media
        .kind
    {
        db::MediaKind::Episode | db::MediaKind::Season => item
            .grandparent
            .as_ref()
            .and_then(|series| series.released_at)
            .or(item
                .media
                .released_at),
        _ => {
            item.media
                .released_at
        }
    };
    if let Some(released_at) = released_at {
        data.insert(
            "Year".to_string(),
            Value::from(chrono::Datelike::year(&released_at.date())),
        );
    }
}

fn put_episode(data: &mut Map<String, Value>, item: &ItemContext) {
    put_series(data, item);
    if let Some(season) = item
        .parent
        .as_ref()
    {
        put(data, "SeasonId", simple_id(&season.id));
    }
    // On an episode row, `parent_idx` is the season number and `idx` the
    // episode number.
    put_padded_number(
        data,
        "SeasonNumber",
        item.media
            .parent_idx,
    );
    put_padded_number(
        data,
        "EpisodeNumber",
        item.media
            .idx,
    );
}

/// A season carries the same series keys as an episode — the plugin's stock
/// template has a dedicated season branch that prints `SeriesName` — and its
/// own `idx` is the season number.
fn put_season(data: &mut Map<String, Value>, item: &ItemContext) {
    put_series(data, item);
    put_padded_number(
        data,
        "SeasonNumber",
        item.media
            .idx,
    );
}

fn put_series(data: &mut Map<String, Value>, item: &ItemContext) {
    if let Some(series) = item
        .grandparent
        .as_ref()
    {
        put(data, "SeriesName", &series.title);
        put(data, "SeriesId", simple_id(&series.id));
    }
}

/// `SeasonNumber`, `SeasonNumber00` and `SeasonNumber000` (and the episode
/// equivalents): the raw number plus its two zero-padded renderings.
fn put_padded_number(data: &mut Map<String, Value>, key: &str, number: Option<i64>) {
    let Some(number) = number else {
        return;
    };
    data.insert(key.to_string(), Value::from(number));
    put(data, &format!("{key}00"), format!("{number:02}"));
    put(data, &format!("{key}000"), format!("{number:03}"));
}

fn put_providers(data: &mut Map<String, Value>, ids: &db::ExternalIds) {
    if let Some(imdb) = ids
        .imdb
        .as_ref()
    {
        put(data, "Provider_imdb", imdb.to_string());
    }
    if let Some(tmdb) = ids.tmdb {
        put(data, "Provider_tmdb", tmdb.to_string());
    }
    if let Some(tvdb) = ids.tvdb {
        put(data, "Provider_tvdb", tvdb.to_string());
    }
}

/// `Video_0_*`, `Audio_0_*`, `Subtitle_0_*`: the index counts per type, so the
/// first audio track is always `Audio_0` whatever its container stream index.
fn put_streams(data: &mut Map<String, Value>, streams: &[MediaStream]) {
    let (mut videos, mut audios, mut subtitles) = (0usize, 0usize, 0usize);
    for stream in streams {
        match stream.type_ {
            Some(MediaStreamType::Video) => {
                let prefix = format!("Video_{videos}");
                videos += 1;
                put_opt(
                    data,
                    &format!("{prefix}_Codec"),
                    stream
                        .codec
                        .as_deref(),
                );
                put_opt_i64(data, &format!("{prefix}_Width"), stream.width);
                put_opt_i64(data, &format!("{prefix}_Height"), stream.height);
                put_opt_i64(data, &format!("{prefix}_Bitrate"), stream.bit_rate);
            }
            Some(MediaStreamType::Audio) => {
                let prefix = format!("Audio_{audios}");
                audios += 1;
                put_opt(
                    data,
                    &format!("{prefix}_Codec"),
                    stream
                        .codec
                        .as_deref(),
                );
                put_opt(
                    data,
                    &format!("{prefix}_Language"),
                    stream
                        .language
                        .as_deref(),
                );
                put_opt_i64(data, &format!("{prefix}_Channels"), stream.channels);
                put_opt_i64(data, &format!("{prefix}_Bitrate"), stream.bit_rate);
            }
            Some(MediaStreamType::Subtitle) => {
                let prefix = format!("Subtitle_{subtitles}");
                subtitles += 1;
                put_opt(
                    data,
                    &format!("{prefix}_Codec"),
                    stream
                        .codec
                        .as_deref(),
                );
                put_opt(
                    data,
                    &format!("{prefix}_Language"),
                    stream
                        .language
                        .as_deref(),
                );
                put_opt(
                    data,
                    &format!("{prefix}_Title"),
                    stream
                        .title
                        .as_deref(),
                );
            }
            // Embedded images, data and lyric streams have no plugin variables.
            _ => {}
        }
    }
}

// --- event ----------------------------------------------------------------

fn put_event(
    data: &mut Map<String, Value>,
    event: &WebhookEvent,
    item: Option<&ItemContext>,
) {
    match event {
        WebhookEvent::Generic { title, extra } => {
            put(data, "Name", title);
            for (key, value) in extra {
                put(data, key, value);
            }
        }
        WebhookEvent::PlaybackStart { playback }
        | WebhookEvent::PlaybackProgress { playback }
        | WebhookEvent::PlaybackStop { playback } => {
            put_user(data, &playback.user);
            put_device(data, &playback.device);
            put_playback(data, playback, item);
        }
        WebhookEvent::AuthenticationSuccess { user, device }
        | WebhookEvent::SessionStart { user, device } => {
            put_user(data, user);
            put_device(data, device);
        }
        WebhookEvent::AuthenticationFailure {
            username,
            remote_ip,
        } => {
            put(data, "NotificationUsername", username);
            put_opt(data, "RemoteIp", remote_ip.as_deref());
        }
        WebhookEvent::TaskCompleted {
            key,
            name,
            succeeded,
            elapsed_ms,
        } => {
            put(data, "TaskName", name);
            put(data, "TaskKey", key);
            data.insert("TaskSucceeded".to_string(), Value::Bool(*succeeded));
            data.insert("TaskElapsedMs".to_string(), Value::from(*elapsed_ms));
        }
        WebhookEvent::UserCreated { user }
        | WebhookEvent::UserUpdated { user }
        | WebhookEvent::UserPasswordChanged { user } => put_user(data, user),
        WebhookEvent::UserDeleted { user_id, username } => {
            put(data, "NotificationUsername", username);
            put(data, "UserId", simple_id(user_id));
        }
        WebhookEvent::UserDataSaved {
            user, save_reason, ..
        } => {
            put_user(data, user);
            put(data, "SaveReason", save_reason.to_string());
        }
        // Everything these two events expose comes from the item itself.
        WebhookEvent::ItemAdded { .. } | WebhookEvent::ItemDeleted { .. } => {}
    }
}

fn put_user(data: &mut Map<String, Value>, user: &UserEventData) {
    put(data, "NotificationUsername", &user.username);
    put(data, "UserId", simple_id(&user.id));
}

fn put_device(data: &mut Map<String, Value>, device: &DeviceEventData) {
    put(data, "DeviceId", &device.id);
    put(data, "DeviceName", &device.name);
    put(data, "ClientName", &device.client_name);
    put_opt(
        data,
        "RemoteIp",
        device
            .remote_ip
            .as_deref(),
    );
}

fn put_playback(
    data: &mut Map<String, Value>,
    playback: &PlaybackEventData,
    item: Option<&ItemContext>,
) {
    data.insert(
        "PlaybackPositionTicks".to_string(),
        Value::from(playback.position_ticks),
    );
    put(
        data,
        "PlaybackPosition",
        hms(playback.position_ticks / TICKS_PER_SECOND),
    );
    data.insert("IsPaused".to_string(), Value::Bool(playback.is_paused));
    if let Some(method) = &playback.play_method {
        put(data, "PlayMethod", method.to_string());
    }
    data.insert(
        "PlayedToCompletion".to_string(),
        Value::Bool(played_to_completion(playback.position_ticks, item)),
    );
}

/// Playback counts as completed at 90 % of the runtime. Without a known
/// runtime there is nothing to compare against, so it never completes.
fn played_to_completion(position_ticks: i64, item: Option<&ItemContext>) -> bool {
    let Some(runtime) = item
        .and_then(|item| {
            item.media
                .runtime
        })
        .filter(|seconds| *seconds > 0)
    else {
        return false;
    };
    position_ticks as f64 >= ticks(runtime) as f64 * COMPLETION_RATIO
}

// --- primitives -----------------------------------------------------------

/// Seconds as ticks. Saturating: a nonsense runtime read out of the database
/// must not overflow (and panic in a debug build) inside the dispatcher.
fn ticks(seconds: i64) -> i64 {
    seconds.saturating_mul(TICKS_PER_SECOND)
}

/// Jellyfin ids carry no dashes.
fn simple_id(id: &Uuid) -> String {
    id.simple()
        .to_string()
}

fn hms(total_seconds: i64) -> String {
    let seconds = total_seconds.max(0);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

fn put(data: &mut Map<String, Value>, key: &str, value: impl AsRef<str>) {
    data.insert(
        key.to_string(),
        Value::String(
            value
                .as_ref()
                .to_string(),
        ),
    );
}

fn put_opt(data: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        put(data, key, value);
    }
}

fn put_opt_i64(data: &mut Map<String, Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        data.insert(key.to_string(), Value::from(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::webhooks::events::UserDataSaveReason;
    use remux_sdks::remux::{
        DiscordMentionType, MediaSourceInfo, MediaStream, MediaStreamType,
        NotificationType, PlayMethod, WebhookItemTypes, WebhookKeyValue,
    };
    use uuid::Uuid;

    fn server() -> ServerInfo {
        ServerInfo {
            id: "server-abc".into(),
            name: "My Server".into(),
            version: "1.2.3".into(),
            url: "https://media.example.test".into(),
        }
    }

    const SERIES_ID: u128 = 10;
    const SEASON_ID: u128 = 11;
    const EPISODE_ID: u128 = 12;

    /// 01:30:45.
    const RUNTIME_SECONDS: i64 = 5445;

    fn probe() -> MediaSourceInfo {
        MediaSourceInfo {
            container: Some("mkv".into()),
            media_streams: vec![
                MediaStream {
                    index: 0,
                    type_: Some(MediaStreamType::Video),
                    codec: Some("h264".into()),
                    width: Some(1920),
                    height: Some(1080),
                    bit_rate: Some(8_000_000),
                    ..Default::default()
                },
                MediaStream {
                    index: 1,
                    type_: Some(MediaStreamType::Audio),
                    codec: Some("aac".into()),
                    language: Some("eng".into()),
                    channels: Some(6),
                    bit_rate: Some(640_000),
                    ..Default::default()
                },
                MediaStream {
                    index: 2,
                    type_: Some(MediaStreamType::Audio),
                    codec: Some("ac3".into()),
                    language: Some("fra".into()),
                    channels: Some(2),
                    bit_rate: Some(192_000),
                    ..Default::default()
                },
                MediaStream {
                    index: 3,
                    type_: Some(MediaStreamType::Subtitle),
                    codec: Some("subrip".into()),
                    language: Some("eng".into()),
                    title: Some("English (SDH)".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    /// S02E05 of a series, with runtime, release date, provider ids and streams.
    fn episode() -> ItemContext {
        ItemContext {
            media: db::Media {
                id: Uuid::from_u128(EPISODE_ID),
                kind: db::MediaKind::Episode,
                title: "The One With The Test".into(),
                description: Some("An episode overview.".into()),
                runtime: Some(RUNTIME_SECONDS),
                released_at: Some(
                    chrono::NaiveDate::from_ymd_opt(2021, 3, 4)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                ),
                idx: Some(5),
                parent_idx: Some(2),
                parent_id: Some(Uuid::from_u128(SEASON_ID)),
                grandparent_id: Some(Uuid::from_u128(SERIES_ID)),
                external_ids: db::ExternalIds {
                    imdb: Some(
                        db::NonEmptyString::try_new("tt1234567".to_string()).unwrap(),
                    ),
                    tmdb: Some(42),
                    tvdb: Some(7),
                    ..Default::default()
                },
                probe_data: Some(probe()),
                ..Default::default()
            },
            parent: Some(db::Media {
                id: Uuid::from_u128(SEASON_ID),
                kind: db::MediaKind::Season,
                title: "Season 2".into(),
                idx: Some(2),
                ..Default::default()
            }),
            grandparent: Some(db::Media {
                id: Uuid::from_u128(SERIES_ID),
                kind: db::MediaKind::Series,
                title: "Test Show".into(),
                ..Default::default()
            }),
            genres: vec!["Drama".into(), "Sci-Fi".into()],
        }
    }

    fn movie() -> ItemContext {
        ItemContext {
            media: db::Media {
                id: Uuid::from_u128(20),
                kind: db::MediaKind::Movie,
                title: "A Movie".into(),
                runtime: Some(RUNTIME_SECONDS),
                ..Default::default()
            },
            parent: None,
            grandparent: None,
            genres: vec![],
        }
    }

    fn item_added() -> WebhookEvent {
        WebhookEvent::ItemAdded {
            item_id: Uuid::from_u128(EPISODE_ID),
        }
    }

    fn user() -> UserEventData {
        UserEventData {
            id: Uuid::from_u128(1),
            username: "alice".into(),
        }
    }

    fn device() -> DeviceEventData {
        DeviceEventData {
            id: "device-1".into(),
            name: "Living Room".into(),
            client_name: "Jellyfin Web".into(),
            remote_ip: Some("10.0.0.2".into()),
        }
    }

    fn playback(position_ticks: i64) -> WebhookEvent {
        WebhookEvent::PlaybackStart {
            playback: PlaybackEventData {
                user: user(),
                item_id: Uuid::from_u128(EPISODE_ID),
                device: device(),
                position_ticks,
                is_paused: true,
                play_method: Some(PlayMethod::DirectStream),
            },
        }
    }

    fn str_at(data: &Map<String, Value>, key: &str) -> String {
        data.get(key)
            .unwrap_or_else(|| panic!("missing key {key}: got {:?}", data.keys()))
            .as_str()
            .unwrap_or_else(|| panic!("key {key} is not a string: {:?}", data[key]))
            .to_string()
    }

    fn hook(destination: WebhookDestination) -> db::Webhook {
        let now = chrono::Utc::now();
        db::Webhook {
            id: Uuid::from_u128(100),
            name: "test".into(),
            enabled: true,
            url: "https://example.test/hook".into(),
            template: "{{Name}}".into(),
            destination,
            notification_types: vec![NotificationType::ItemAdded],
            user_filter: vec![],
            item_types: WebhookItemTypes::default(),
            send_all_properties: false,
            trim_whitespace: false,
            skip_empty_message_body: false,
            created_at: now,
            updated_at: now,
        }
    }

    // --- common variables -------------------------------------------------

    #[test]
    fn common_variables_use_the_plugin_key_names() {
        let data = build_data(&server(), &item_added(), None);
        assert_eq!(str_at(&data, "ServerId"), "server-abc");
        assert_eq!(str_at(&data, "ServerName"), "My Server");
        assert_eq!(str_at(&data, "ServerVersion"), "1.2.3");
        assert_eq!(str_at(&data, "ServerUrl"), "https://media.example.test");
        assert_eq!(str_at(&data, "NotificationType"), "ItemAdded");

        for key in ["Timestamp", "UtcTimestamp"] {
            let raw = str_at(&data, key);
            chrono::DateTime::parse_from_rfc3339(&raw)
                .unwrap_or_else(|e| panic!("{key} = {raw:?} is not RFC3339: {e}"));
        }
    }

    // --- item variables ---------------------------------------------------

    #[test]
    fn item_variables_use_the_plugin_key_names() {
        let item = episode();
        let data = build_data(&server(), &item_added(), Some(&item));
        assert_eq!(str_at(&data, "Name"), "The One With The Test");
        assert_eq!(str_at(&data, "Overview"), "An episode overview.");
        assert_eq!(str_at(&data, "ItemType"), "Episode");
        assert_eq!(str_at(&data, "Genres"), "Drama, Sci-Fi");
        assert_eq!(data["Year"], Value::from(2021));
        assert_eq!(str_at(&data, "PremiereDate"), "2021-03-04");
    }

    /// Jellyfin ids carry no dashes.
    #[test]
    fn item_id_is_the_dashless_uuid() {
        let item = episode();
        let data = build_data(&server(), &item_added(), Some(&item));
        let id = str_at(&data, "ItemId");
        assert_eq!(
            id,
            Uuid::from_u128(EPISODE_ID)
                .simple()
                .to_string()
        );
        assert!(!id.contains('-'), "ItemId must not contain dashes: {id}");
    }

    #[test]
    fn item_type_maps_every_media_kind() {
        let cases = [
            (db::MediaKind::Movie, "Movie"),
            (db::MediaKind::Episode, "Episode"),
            (db::MediaKind::Series, "Series"),
            (db::MediaKind::Season, "Season"),
            (db::MediaKind::Album, "MusicAlbum"),
            (db::MediaKind::Track, "Audio"),
            (db::MediaKind::Stream, "Video"),
            (db::MediaKind::TvChannel, "Video"),
        ];
        for (kind, expected) in cases {
            let item = ItemContext {
                media: db::Media {
                    kind: kind.clone(),
                    ..movie().media
                },
                ..movie()
            };
            let data = build_data(&server(), &item_added(), Some(&item));
            assert_eq!(
                str_at(&data, "ItemType"),
                expected,
                "wrong ItemType for {kind:?}"
            );
        }
    }

    #[test]
    fn runtime_is_exposed_as_ticks_and_hms() {
        let item = episode();
        let data = build_data(&server(), &item_added(), Some(&item));
        assert_eq!(
            data["RunTimeTicks"],
            Value::from(RUNTIME_SECONDS * TICKS_PER_SECOND)
        );
        assert_eq!(data["RunTimeTicks"], Value::from(54_450_000_000i64));
        assert_eq!(str_at(&data, "RunTime"), "01:30:45");
    }

    /// Imported templates print the runtime unconditionally, so the keys are
    /// always present — zeroed rather than missing when it is unknown.
    #[test]
    fn runtime_variables_fall_back_to_zero() {
        let item = ItemContext {
            media: db::Media {
                runtime: None,
                ..movie().media
            },
            ..movie()
        };
        let data = build_data(&server(), &item_added(), Some(&item));
        assert_eq!(data["RunTimeTicks"], Value::from(0));
        assert_eq!(str_at(&data, "RunTime"), "00:00:00");
    }

    // --- episode variables ------------------------------------------------

    #[test]
    fn episode_numbers_are_zero_padded() {
        let item = episode();
        let data = build_data(&server(), &item_added(), Some(&item));
        assert_eq!(data["SeasonNumber"], Value::from(2));
        assert_eq!(str_at(&data, "SeasonNumber00"), "02");
        assert_eq!(str_at(&data, "SeasonNumber000"), "002");
        assert_eq!(data["EpisodeNumber"], Value::from(5));
        assert_eq!(str_at(&data, "EpisodeNumber00"), "05");
        assert_eq!(str_at(&data, "EpisodeNumber000"), "005");
    }

    #[test]
    fn episode_links_series_and_season() {
        let item = episode();
        let data = build_data(&server(), &item_added(), Some(&item));
        assert_eq!(str_at(&data, "SeriesName"), "Test Show");
        assert_eq!(
            str_at(&data, "SeriesId"),
            Uuid::from_u128(SERIES_ID)
                .simple()
                .to_string()
        );
        assert_eq!(
            str_at(&data, "SeasonId"),
            Uuid::from_u128(SEASON_ID)
                .simple()
                .to_string()
        );
    }

    /// The plugin reads `Year` off the *series* for an episode, not off the
    /// episode's own air date.
    #[test]
    fn episode_year_comes_from_the_series() {
        let base = episode();
        let item = ItemContext {
            grandparent: Some(db::Media {
                released_at: Some(
                    chrono::NaiveDate::from_ymd_opt(2019, 9, 1)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                ),
                ..base
                    .grandparent
                    .clone()
                    .unwrap()
            }),
            ..base
        };
        let data = build_data(&server(), &item_added(), Some(&item));
        assert_eq!(
            data["Year"],
            Value::from(2019),
            "Year must be the series' production year"
        );
        // The episode's own air date still drives PremiereDate.
        assert_eq!(str_at(&data, "PremiereDate"), "2021-03-04");
    }

    /// The plugin's stock template has a dedicated Season branch that prints
    /// the series name and the season number.
    #[test]
    fn season_gets_the_series_keys_and_its_own_number() {
        let item = ItemContext {
            media: db::Media {
                id: Uuid::from_u128(SEASON_ID),
                kind: db::MediaKind::Season,
                title: "Season 2".into(),
                // A season's own `idx` is the season number.
                idx: Some(2),
                grandparent_id: Some(Uuid::from_u128(SERIES_ID)),
                ..Default::default()
            },
            parent: None,
            grandparent: Some(db::Media {
                id: Uuid::from_u128(SERIES_ID),
                kind: db::MediaKind::Series,
                title: "Test Show".into(),
                released_at: Some(
                    chrono::NaiveDate::from_ymd_opt(2019, 9, 1)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                ),
                ..Default::default()
            }),
            genres: vec![],
        };
        let data = build_data(&server(), &item_added(), Some(&item));

        assert_eq!(str_at(&data, "ItemType"), "Season");
        assert_eq!(str_at(&data, "SeriesName"), "Test Show");
        assert_eq!(
            str_at(&data, "SeriesId"),
            Uuid::from_u128(SERIES_ID)
                .simple()
                .to_string()
        );
        assert_eq!(data["SeasonNumber"], Value::from(2));
        assert_eq!(str_at(&data, "SeasonNumber00"), "02");
        assert_eq!(str_at(&data, "SeasonNumber000"), "002");
        assert_eq!(data["Year"], Value::from(2019));
        assert!(
            !data.contains_key("EpisodeNumber"),
            "a season has no episode number"
        );
    }

    #[test]
    fn track_exposes_album_and_artist() {
        let item = ItemContext {
            media: db::Media {
                id: Uuid::from_u128(30),
                kind: db::MediaKind::Track,
                title: "A Song".into(),
                ..Default::default()
            },
            parent: Some(db::Media {
                kind: db::MediaKind::Album,
                title: "An Album".into(),
                ..Default::default()
            }),
            grandparent: Some(db::Media {
                kind: db::MediaKind::Artist,
                title: "An Artist".into(),
                ..Default::default()
            }),
            genres: vec![],
        };
        let data = build_data(&server(), &item_added(), Some(&item));
        assert_eq!(str_at(&data, "Album"), "An Album");
        assert_eq!(str_at(&data, "Artist"), "An Artist");
    }

    // --- providers --------------------------------------------------------

    #[test]
    fn provider_ids_are_exposed() {
        let item = episode();
        let data = build_data(&server(), &item_added(), Some(&item));
        assert_eq!(str_at(&data, "Provider_imdb"), "tt1234567");
        assert_eq!(str_at(&data, "Provider_tmdb"), "42");
        assert_eq!(str_at(&data, "Provider_tvdb"), "7");
    }

    #[test]
    fn absent_provider_ids_produce_no_keys() {
        let item = movie();
        let data = build_data(&server(), &item_added(), Some(&item));
        for key in ["Provider_imdb", "Provider_tmdb", "Provider_tvdb"] {
            assert!(!data.contains_key(key), "{key} must be absent");
        }
    }

    // --- streams ----------------------------------------------------------

    #[test]
    fn stream_variables_are_indexed_per_type() {
        let item = episode();
        let data = build_data(&server(), &item_added(), Some(&item));

        assert_eq!(str_at(&data, "Video_0_Codec"), "h264");
        assert_eq!(data["Video_0_Width"], Value::from(1920));
        assert_eq!(data["Video_0_Height"], Value::from(1080));
        assert_eq!(data["Video_0_Bitrate"], Value::from(8_000_000));

        assert_eq!(str_at(&data, "Audio_0_Codec"), "aac");
        assert_eq!(str_at(&data, "Audio_0_Language"), "eng");
        assert_eq!(data["Audio_0_Channels"], Value::from(6));
        assert_eq!(data["Audio_0_Bitrate"], Value::from(640_000));

        // The index counts per type, not the raw media stream index: the second
        // audio track is Audio_1 even though its stream index is 2.
        assert_eq!(str_at(&data, "Audio_1_Codec"), "ac3");
        assert_eq!(str_at(&data, "Audio_1_Language"), "fra");
        assert!(
            !data.contains_key("Audio_2_Codec"),
            "only two audio tracks exist"
        );

        assert_eq!(str_at(&data, "Subtitle_0_Codec"), "subrip");
        assert_eq!(str_at(&data, "Subtitle_0_Language"), "eng");
        assert_eq!(str_at(&data, "Subtitle_0_Title"), "English (SDH)");
    }

    #[test]
    fn stream_variables_are_absent_without_probe_data() {
        let item = movie();
        let data = build_data(&server(), &item_added(), Some(&item));
        for key in ["Video_0_Codec", "Audio_0_Codec", "Subtitle_0_Codec"] {
            assert!(!data.contains_key(key), "{key} must be absent");
        }
    }

    // --- user / device / playback -----------------------------------------

    #[test]
    fn user_variables_use_the_plugin_key_names() {
        let data =
            build_data(&server(), &WebhookEvent::UserCreated { user: user() }, None);
        assert_eq!(str_at(&data, "NotificationUsername"), "alice");
        assert_eq!(
            str_at(&data, "UserId"),
            Uuid::from_u128(1)
                .simple()
                .to_string()
        );
    }

    #[test]
    fn playback_variables_use_the_plugin_key_names() {
        let item = episode();
        // 00:10:00 into the episode.
        let data =
            build_data(&server(), &playback(600 * TICKS_PER_SECOND), Some(&item));
        assert_eq!(str_at(&data, "DeviceId"), "device-1");
        assert_eq!(str_at(&data, "DeviceName"), "Living Room");
        assert_eq!(str_at(&data, "ClientName"), "Jellyfin Web");
        assert_eq!(
            data["PlaybackPositionTicks"],
            Value::from(600 * TICKS_PER_SECOND)
        );
        assert_eq!(str_at(&data, "PlaybackPosition"), "00:10:00");
        assert_eq!(data["IsPaused"], Value::Bool(true));
        assert_eq!(str_at(&data, "PlayMethod"), "DirectStream");
        assert_eq!(str_at(&data, "NotificationUsername"), "alice");
    }

    /// 90 % of the runtime is the threshold, inclusive.
    #[test]
    fn played_to_completion_flips_at_ninety_percent() {
        let item = episode();
        let full = RUNTIME_SECONDS * TICKS_PER_SECOND;
        let cases = [
            (0, false),
            (full / 2, false),
            (full * 89 / 100, false),
            (full * 90 / 100, true),
            (full, true),
        ];
        for (position, expected) in cases {
            let data = build_data(&server(), &playback(position), Some(&item));
            assert_eq!(
                data["PlayedToCompletion"],
                Value::Bool(expected),
                "position {position} of {full}"
            );
        }
    }

    #[test]
    fn played_to_completion_is_false_without_a_runtime() {
        let item = ItemContext {
            media: db::Media {
                runtime: None,
                ..movie().media
            },
            ..movie()
        };
        let data = build_data(&server(), &playback(i64::MAX / 2), Some(&item));
        assert_eq!(data["PlayedToCompletion"], Value::Bool(false));
    }

    #[test]
    fn playback_variables_are_absent_for_non_playback_events() {
        let data = build_data(&server(), &item_added(), Some(&episode()));
        for key in [
            "PlaybackPosition",
            "PlaybackPositionTicks",
            "IsPaused",
            "PlayMethod",
            "PlayedToCompletion",
            "DeviceId",
        ] {
            assert!(!data.contains_key(key), "{key} must be absent");
        }
    }

    // --- task / auth ------------------------------------------------------

    #[test]
    fn task_completed_variables_use_the_plugin_key_names() {
        let data = build_data(
            &server(),
            &WebhookEvent::TaskCompleted {
                key: "scan".into(),
                name: "Scan library".into(),
                succeeded: true,
                elapsed_ms: 4242,
            },
            None,
        );
        assert_eq!(str_at(&data, "TaskName"), "Scan library");
        assert_eq!(str_at(&data, "TaskKey"), "scan");
        assert_eq!(data["TaskSucceeded"], Value::Bool(true));
        assert_eq!(data["TaskElapsedMs"], Value::from(4242));
    }

    #[test]
    fn authentication_failure_exposes_username_and_remote_ip() {
        let data = build_data(
            &server(),
            &WebhookEvent::AuthenticationFailure {
                username: "mallory".into(),
                remote_ip: Some("10.0.0.9".into()),
            },
            None,
        );
        assert_eq!(str_at(&data, "NotificationUsername"), "mallory");
        assert_eq!(str_at(&data, "RemoteIp"), "10.0.0.9");
        assert!(
            !data.contains_key("UserId"),
            "a failed login has no user id"
        );
    }

    #[test]
    fn user_data_saved_exposes_the_save_reason() {
        let data = build_data(
            &server(),
            &WebhookEvent::UserDataSaved {
                user: user(),
                item_id: Uuid::from_u128(EPISODE_ID),
                save_reason: UserDataSaveReason::PlaybackFinished,
            },
            None,
        );
        assert_eq!(str_at(&data, "SaveReason"), "PlaybackFinished");
    }

    #[test]
    fn generic_event_exposes_its_title_and_extra_pairs() {
        let data = build_data(
            &server(),
            &WebhookEvent::Generic {
                title: "Something happened".into(),
                extra: vec![("Detail".into(), "42".into())],
            },
            None,
        );
        assert_eq!(str_at(&data, "Name"), "Something happened");
        assert_eq!(str_at(&data, "Detail"), "42");
    }

    // --- per-hook fields --------------------------------------------------

    #[test]
    fn generic_destination_fields_are_merged() {
        let base = build_data(&server(), &item_added(), Some(&episode()));
        let hook = hook(WebhookDestination::Generic {
            headers: vec![],
            fields: vec![
                WebhookKeyValue {
                    key: "channel".into(),
                    value: "#general".into(),
                },
                WebhookKeyValue {
                    key: "kind".into(),
                    value: "alert".into(),
                },
            ],
        });

        let merged = with_hook_fields(&base, &hook);
        assert_eq!(str_at(&merged, "channel"), "#general");
        assert_eq!(str_at(&merged, "kind"), "alert");
        // The common dictionary survives the overlay.
        assert_eq!(str_at(&merged, "Name"), "The One With The Test");
        // …and the overlay does not mutate it.
        assert!(!base.contains_key("channel"));
    }

    /// `GenericClient.SendAsync` skips a field when either half is empty — the
    /// same rule its header loop applies.
    #[test]
    fn generic_destination_fields_skip_empty_halves() {
        let base = build_data(&server(), &item_added(), Some(&episode()));
        let hook = hook(WebhookDestination::Generic {
            headers: vec![],
            fields: vec![
                WebhookKeyValue {
                    key: "".into(),
                    value: "orphan".into(),
                },
                WebhookKeyValue {
                    key: "blank".into(),
                    value: "".into(),
                },
                WebhookKeyValue {
                    key: "channel".into(),
                    value: "#general".into(),
                },
            ],
        });

        let merged = with_hook_fields(&base, &hook);
        assert_eq!(str_at(&merged, "channel"), "#general");
        assert!(!merged.contains_key(""), "an empty key must be skipped");
        assert!(
            !merged.contains_key("blank"),
            "an empty value must be skipped"
        );
    }

    #[test]
    fn a_generic_hook_with_no_fields_borrows_the_dictionary() {
        let base = build_data(&server(), &item_added(), Some(&episode()));
        let merged = with_hook_fields(
            &base,
            &hook(WebhookDestination::Generic {
                headers: vec![],
                fields: vec![],
            }),
        );
        assert_eq!(merged.len(), base.len());
        assert!(matches!(merged, Cow::Borrowed(_)), "no clone is needed");
    }

    // --- discord destination variables -------------------------------------

    fn discord_with(
        avatar_url: Option<&str>,
        bot_username: Option<&str>,
        embed_color: Option<&str>,
        mention_type: DiscordMentionType,
    ) -> db::Webhook {
        hook(WebhookDestination::Discord {
            avatar_url: avatar_url.map(str::to_string),
            bot_username: bot_username.map(str::to_string),
            embed_color: embed_color.map(str::to_string),
            mention_type,
        })
    }

    fn discord_vars(hook: &db::Webhook) -> Map<String, Value> {
        let base = build_data(&server(), &item_added(), Some(&episode()));
        with_hook_fields(&base, hook).into_owned()
    }

    /// `DiscordClient.SendAsync` always sets `MentionType`, empty for `None`.
    /// This is what `{{MentionType}}` in a plugin template resolves against.
    #[test]
    fn discord_always_exposes_the_mention_type() {
        for (mention_type, expected) in [
            (DiscordMentionType::None, ""),
            (DiscordMentionType::Here, "@here"),
            (DiscordMentionType::Everyone, "@everyone"),
        ] {
            let data = discord_vars(&discord_with(None, None, None, mention_type));
            assert_eq!(
                str_at(&data, "MentionType"),
                expected,
                "{mention_type:?} must render as {expected:?}"
            );
        }
    }

    /// The plugin sets a username under **both** `Username` and `BotUsername`,
    /// and only when it is non-empty. Its stock templates use `{{BotUsername}}`.
    #[test]
    fn discord_exposes_the_bot_identity_under_the_plugin_keys() {
        let data = discord_vars(&discord_with(
            Some("https://example.test/a.png"),
            Some("remux"),
            Some("#AA5CC3"),
            DiscordMentionType::None,
        ));
        assert_eq!(str_at(&data, "AvatarUrl"), "https://example.test/a.png");
        assert_eq!(str_at(&data, "Username"), "remux");
        assert_eq!(str_at(&data, "BotUsername"), "remux");
    }

    /// Presence parity: the plugin only inserts these keys when configured, so
    /// an unset one must be *missing*, not present-and-empty — that is what
    /// makes `{{#if_exist AvatarUrl}}` behave as it does in the plugin.
    #[test]
    fn discord_omits_the_unset_identity_options() {
        for hook in [
            discord_with(None, None, None, DiscordMentionType::None),
            discord_with(Some(""), Some(""), Some(""), DiscordMentionType::None),
        ] {
            let data = discord_vars(&hook);
            for key in ["AvatarUrl", "Username", "BotUsername"] {
                assert!(
                    !data.contains_key(key),
                    "{key} must be absent when it is not configured"
                );
            }
            // …but the mention type is always there.
            assert!(data.contains_key("MentionType"));
        }
    }

    /// The plugin formats the colour into an integer before it reaches the
    /// template (`FormatColorCode`), so `{{EmbedColor}}` is a number.
    #[test]
    fn discord_exposes_the_embed_color_as_an_integer() {
        let data = discord_vars(&discord_with(
            None,
            None,
            Some("#AA5CC3"),
            DiscordMentionType::None,
        ));
        assert_eq!(data["EmbedColor"], Value::from(11_164_867));
    }

    /// Intended deviation from the plugin, which omits the key: the stock
    /// `Discord.handlebars` interpolates `{{EmbedColor}}` bare, so an absent
    /// key renders `"color": ""` and Discord rejects the payload. The
    /// invariant belongs here, not in a dashboard form three crates away.
    #[test]
    fn discord_always_exposes_an_embed_color() {
        for embed_color in [None, Some(""), Some("nonsense")] {
            let data = discord_vars(&discord_with(
                None,
                None,
                embed_color,
                DiscordMentionType::None,
            ));
            assert_eq!(
                data["EmbedColor"],
                Value::from(DEFAULT_EMBED_COLOR),
                "{embed_color:?} must still yield a usable colour"
            );
        }
    }

    /// A `Generic` hook must not gain Discord keys, and vice versa.
    #[test]
    fn discord_variables_are_not_exposed_to_generic_hooks() {
        let data = with_hook_fields(
            &build_data(&server(), &item_added(), Some(&episode())),
            &hook(WebhookDestination::Generic {
                headers: vec![],
                fields: vec![WebhookKeyValue {
                    key: "channel".into(),
                    value: "#general".into(),
                }],
            }),
        )
        .into_owned();
        for key in ["MentionType", "AvatarUrl", "Username", "BotUsername"] {
            assert!(!data.contains_key(key), "{key} is Discord-only");
        }
    }

    // --- parse_embed_color -------------------------------------------------

    #[test]
    fn parse_embed_color_reads_a_six_digit_hex() {
        assert_eq!(parse_embed_color("#AA5CC3"), 11_164_867);
        // Lower case and a missing '#' are both accepted.
        assert_eq!(parse_embed_color("aa5cc3"), 11_164_867);
        assert_eq!(parse_embed_color("#000000"), 0);
        assert_eq!(parse_embed_color("#FFFFFF"), 16_777_215);
    }

    /// The plugin's `FormatColorCode` slices `hexCode[1..6]` and drops the last
    /// digit, so `#AA5CC3` reaches Discord as 697 804. That bug is not ours.
    #[test]
    fn parse_embed_color_does_not_reproduce_the_plugin_truncation() {
        assert_ne!(parse_embed_color("#AA5CC3"), 697_804);
    }

    #[test]
    fn parse_embed_color_falls_back_to_the_default() {
        for input in [
            "",
            "#",
            "#AA5CC",   // too short
            "#AA5CC3F", // too long
            "#GGGGGG",  // not hex
            "rebeccapurple",
        ] {
            assert_eq!(
                parse_embed_color(input),
                DEFAULT_EMBED_COLOR,
                "{input:?} must fall back to the default colour"
            );
        }
    }

    // --- enrich_item (against a real database) -----------------------------

    const SERIES_IMDB: &str = "tt5550001";

    fn imdb(value: &str) -> db::NonEmptyString {
        db::NonEmptyString::try_new(value.to_string()).unwrap()
    }

    /// `Media::save` validates that the row id is the one derived from its
    /// external ids, so the fixtures have to be keyed the same way.
    fn derived_id(
        kind: db::MediaKind,
        external_ids: &db::ExternalIds,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Uuid {
        Uuid::from(&db::MediaIdRaw {
            kind,
            external_ids: external_ids.clone(),
            season,
            episode,
        })
    }

    /// Inserts a `Test Show` / `Season 2` pair and returns
    /// `(series, season, unsaved S02E05 episode)`.
    async fn seed_show(ctx: &AppContext) -> (db::Media, db::Media, db::Media) {
        let series_ids = db::ExternalIds {
            imdb: Some(imdb(SERIES_IMDB)),
            ..Default::default()
        };
        let child_ids = db::ExternalIds {
            series_imdb: Some(imdb(SERIES_IMDB)),
            ..Default::default()
        };

        let mut series = db::Media {
            id: derived_id(db::MediaKind::Series, &series_ids, None, None),
            kind: db::MediaKind::Series,
            title: "Test Show".into(),
            external_ids: series_ids,
            ..Default::default()
        };
        series
            .save(&ctx.db)
            .await
            .expect("series must insert");

        let mut season = db::Media {
            id: derived_id(db::MediaKind::Season, &child_ids, Some(2), None),
            kind: db::MediaKind::Season,
            title: "Season 2".into(),
            idx: Some(2),
            parent_id: Some(series.id),
            grandparent_id: Some(series.id),
            external_ids: child_ids.clone(),
            ..Default::default()
        };
        season
            .save(&ctx.db)
            .await
            .expect("season must insert");

        let episode = db::Media {
            id: derived_id(db::MediaKind::Episode, &child_ids, Some(2), Some(5)),
            kind: db::MediaKind::Episode,
            title: "The One With The Test".into(),
            idx: Some(5),
            parent_idx: Some(2),
            parent_id: Some(season.id),
            grandparent_id: Some(series.id),
            external_ids: child_ids,
            ..Default::default()
        };

        (series, season, episode)
    }

    /// Pins the parent/grandparent assignment, which nothing else covers: with
    /// the two swapped, `SeriesName` would render the season title on every
    /// episode webhook and every hand-built `ItemContext` test would stay green.
    #[tokio::test]
    async fn enrich_item_resolves_the_season_as_parent_and_the_series_as_grandparent() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (series, season, mut episode) = seed_show(ctx).await;
        episode
            .save(&ctx.db)
            .await
            .expect("episode must insert");

        let item = enrich_item(
            ctx,
            &WebhookEvent::ItemAdded {
                item_id: episode.id,
            },
        )
        .await
        .expect("the item must be resolved");

        assert_eq!(
            item.media
                .id,
            episode.id
        );
        let parent = item
            .parent
            .as_ref()
            .expect("an episode has a season");
        let grandparent = item
            .grandparent
            .as_ref()
            .expect("an episode has a series");
        assert_eq!(parent.id, season.id, "parent must be the season");
        assert_eq!(parent.kind, db::MediaKind::Season);
        assert_eq!(grandparent.id, series.id, "grandparent must be the series");
        assert_eq!(grandparent.kind, db::MediaKind::Series);

        // The consequence a swap would produce, asserted end to end.
        let data = build_data(
            &server(),
            &WebhookEvent::ItemAdded {
                item_id: episode.id,
            },
            Some(&item),
        );
        assert_eq!(str_at(&data, "SeriesName"), "Test Show");
        assert_eq!(
            str_at(&data, "SeasonId"),
            season
                .id
                .simple()
                .to_string()
        );
        assert_eq!(
            str_at(&data, "SeriesId"),
            series
                .id
                .simple()
                .to_string()
        );
    }

    #[tokio::test]
    async fn enrich_item_returns_none_for_an_unknown_or_itemless_event() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        assert!(
            enrich_item(
                ctx,
                &WebhookEvent::ItemAdded {
                    item_id: Uuid::from_u128(999),
                }
            )
            .await
            .is_none(),
            "an item that is not in the database resolves to nothing"
        );
        assert!(
            enrich_item(ctx, &WebhookEvent::UserCreated { user: user() })
                .await
                .is_none(),
            "an event with no item resolves to nothing"
        );
    }

    /// `ItemDeleted` must read the row off the event — the DB row is already
    /// gone by the time the dispatcher sees it — while still resolving the
    /// parents, which are not deleted.
    #[tokio::test]
    async fn enrich_item_uses_the_row_embedded_in_item_deleted() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        // The episode is deliberately never saved: it stands for a row that has
        // just been deleted.
        let (series, season, episode) = seed_show(ctx).await;

        assert!(
            enrich_item(
                ctx,
                &WebhookEvent::ItemAdded {
                    item_id: episode.id
                }
            )
            .await
            .is_none(),
            "guard: the episode row really is absent from the database"
        );

        let item = enrich_item(
            ctx,
            &WebhookEvent::ItemDeleted {
                item: Box::new(episode.clone()),
            },
        )
        .await
        .expect("the embedded row must be used");

        assert_eq!(
            item.media
                .title,
            "The One With The Test"
        );
        assert_eq!(
            item.parent
                .as_ref()
                .map(|p| p.id),
            Some(season.id)
        );
        assert_eq!(
            item.grandparent
                .as_ref()
                .map(|g| g.id),
            Some(series.id)
        );
    }

    /// `ServerUrl` comes from `Config::public_url`.
    #[tokio::test]
    async fn server_info_reads_the_public_url_from_config() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        assert_eq!(
            ServerInfo::load(ctx)
                .await
                .url,
            "",
            "unset public_url renders as empty, never as a guess"
        );

        let configured = AppContext {
            config: crate::Config {
                public_url: Some("  https://media.example.com  ".into()),
                ..ctx
                    .config
                    .clone()
            },
            ..ctx.clone()
        };
        let info = ServerInfo::load(&configured).await;
        assert_eq!(info.url, "https://media.example.com");
        assert!(
            !info
                .name
                .is_empty(),
            "ServerName must never be empty"
        );
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }
}
