//! Which streams and addons may be read for pictures.

use super::*;

/// The checks that need only the stream row. Used both to advertise pictures
/// and before a pass.
pub fn eligible_row(
    media: &db::Media,
    info: &StreamInfo,
    runtime_s: Option<u64>,
) -> Result<(), Ineligible> {
    let under_channel = media
        .parent
        .as_deref()
        .is_some_and(db::Media::is_live);
    if media.is_live() || under_channel {
        return Err(Ineligible::LiveTv);
    }
    if matches!(info.descriptor, StreamDescriptor::Rtsp { .. }) {
        return Err(Ineligible::Rtsp);
    }
    if info
        .catchup_source
        .is_some()
    {
        return Err(Ineligible::Catchup);
    }
    if info.is_p2p() {
        return Err(Ineligible::Torrent);
    }
    let hls_container = media
        .probe_data
        .as_ref()
        .and_then(|p| {
            p.container
                .as_ref()
        })
        .is_some_and(|c| c.is_hls_input());
    let hls_url = info
        .descriptor
        .as_http_url()
        .is_some_and(crate::playback::engine::is_hls_input_url);
    if hls_container || hls_url {
        return Err(Ineligible::Hls);
    }
    length_ok(runtime_s)
}

/// Whether a length in whole seconds gets pictures. A pass ([`eligible_row`])
/// and the `Trickplay` field ([`manifest_entry`]) both judge by it, so a file
/// a few tenths of a second past a limit is judged the same way by both.
pub(super) fn length_ok(runtime_s: Option<u64>) -> Result<(), Ineligible> {
    match runtime_s {
        None => Err(Ineligible::NoRuntime),
        Some(s) if s < MIN_RUNTIME_SECS => Err(Ineligible::TooShort),
        Some(s) if s > MAX_RUNTIME_SECS => Err(Ineligible::TooLong),
        Some(_) => Ok(()),
    }
}

/// Whether a stored item of this kind may get pictures: a movie, an episode,
/// or a stream or stream group (see [`manifest`]). Never music.
pub fn kind_has_pictures(kind: &db::MediaKind) -> bool {
    matches!(
        kind,
        db::MediaKind::Movie
            | db::MediaKind::Episode
            | db::MediaKind::Stream
            | db::MediaKind::StreamGroup
    )
}

/// Whether an addon's resources make it one kept for seek previews only:
/// "seek previews" on and "stream" off. No viewer is offered its streams: it
/// is asked for them in the background, after a play starts (see
/// [`PreviewFinder`]), and they are kept apart from the title's versions (see
/// [`db::PreviewSource`]).
pub fn previews_only(resources: &[ResourceType]) -> bool {
    resources.contains(&ResourceType::SeekPreviews)
        && !resources.contains(&ResourceType::Stream)
}

/// Whether an addon row is one of the IPTV kinds. A stored row carries its
/// preset id (`iptv-m3u`, `iptv-xtream`); `iptv` is the runtime kind's id.
pub(super) fn is_iptv_kind(kind: &str) -> bool {
    matches!(kind, "iptv" | "iptv-m3u" | "iptv-xtream")
}

/// The checks that need the addon that produced the stream, before a pass
/// (see [`AddonRule`]). A stream with no addon is judged by its row alone; a
/// stream whose addon was deleted is refused where it is read.
pub fn eligible_pass(addon: Option<&Addon>) -> Result<(), Ineligible> {
    let Some(addon) = addon else {
        return Ok(());
    };
    let rule = AddonRule::new(addon);
    if rule.iptv {
        return Err(Ineligible::Iptv);
    }
    if rule.redirect {
        return Err(Ineligible::Redirect);
    }
    if !rule.reads {
        return Err(Ineligible::AddonExcluded);
    }
    Ok(())
}

/// Who may be read for pictures, by one addon's switches, kept in memory so
/// item builders run no query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonRule {
    pub iptv: bool,
    /// "Bypass Remux proxy" while "stream" is on. Its control is hidden with
    /// "stream" off, so a stored value does not act then.
    pub redirect: bool,
    /// Enabled, "seek previews" on, and not an IPTV kind: its streams may be
    /// read for pictures.
    pub reads: bool,
    /// `reads`, with "stream" off (see [`previews_only`]).
    pub preview_only: bool,
    /// Its place in the addon order; preview sources are read in it.
    pub priority: i64,
    /// Asked for users with no addon list of their own (see [`in_scope`]).
    pub is_default: bool,
    /// Asked for every user.
    pub system: bool,
}

impl AddonRule {
    pub fn new(addon: &Addon) -> Self {
        let iptv = is_iptv_kind(
            &addon
                .preset
                .kind,
        );
        let stream = addon
            .resources
            .contains(&ResourceType::Stream);
        let reads = addon.enabled
            && !iptv
            && addon
                .resources
                .contains(&ResourceType::SeekPreviews);
        Self {
            iptv,
            redirect: addon.http_redirect_stream && stream,
            reads,
            preview_only: reads && !stream,
            priority: addon.priority,
            is_default: addon.is_default,
            system: addon.system,
        }
    }

    /// Whether this addon's streams may be read for pictures.
    pub fn allows(&self) -> bool {
        self.reads && !self.redirect
    }
}

/// Every stored addon's [`AddonRule`], by addon id.
pub type AddonRules = HashMap<Uuid, AddonRule>;

/// Whether the addon that produced a stream lets it be read. A stream with no
/// addon is judged by its row alone; one whose addon is not in `rules` (it
/// was deleted) is not read.
pub fn addon_ok(rules: &AddonRules, info: &StreamInfo) -> bool {
    match info.addon_id {
        None => true,
        Some(id) => rules
            .get(&id)
            .is_some_and(AddonRule::allows),
    }
}

/// Whether some enabled addon is kept for seek previews only.
pub fn has_preview_source(rules: &AddonRules) -> bool {
    rules
        .values()
        .any(|rule| rule.preview_only)
}

/// Whether a user is asked an addon's streams: always for a system addon,
/// for a user with an addon list of their own when it holds the addon, and
/// otherwise when the addon is a default one. A deleted addon is not.
pub fn in_scope(
    rules: &AddonRules,
    addon_id: Uuid,
    limited_to: Option<&[Uuid]>,
) -> bool {
    rules
        .get(&addon_id)
        .is_some_and(|rule| {
            rule.system
                || match limited_to {
                    Some(ids) => ids.contains(&addon_id),
                    None => rule.is_default,
                }
        })
}
