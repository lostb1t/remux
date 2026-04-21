pub mod lyrics;
pub mod meta;
pub mod music;
pub mod search;
pub mod stream;

/// Extra args from `YTDLP_EXTRA_ARGS` env var, split on whitespace.
pub(crate) fn ytdlp_extra_args() -> Vec<String> {
    std::env::var("YTDLP_EXTRA_ARGS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

pub use lyrics::{LyricProvider, LyricSearchRequest, LyricService};
pub use meta::*;
pub use music::{MusicMetaProvider, MusicMetaProviderService, MusicMetaResult};
pub use search::{AioSearchService, SearchService, SearchServiceManager, YtDlpSearchService};
pub use stream::{
    AioStreamService, StreamOption, StreamService, StreamServiceManager, YtDlpStreamService,
};
