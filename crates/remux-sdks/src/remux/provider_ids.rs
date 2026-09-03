use std::str::FromStr;

/// Parsed Emby `AnyProviderIdEquals` tokens.
///
/// Clients such as Infuse, UHF, and EPlayer look up library items by
/// `Tmdb.{id}`, `Imdb.tt…`, and `Tvdb.{id}`. Matching is OR across tokens.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
pub struct AnyProviderIds {
    pub tmdb: Vec<i64>,
    pub imdb: Vec<String>,
    pub tvdb: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumString)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
enum ProviderAlias {
    #[strum(serialize = "tmdb", serialize = "themoviedb", serialize = "tmdbid")]
    Tmdb,
    #[strum(serialize = "imdb", serialize = "imdbid")]
    Imdb,
    #[strum(serialize = "tvdb", serialize = "thetvdb", serialize = "tvdbid")]
    Tvdb,
}

impl AnyProviderIds {
    pub fn parse(tokens: &[String]) -> Self {
        let mut ids = Self::default();
        for token in tokens {
            ids.push_token(token);
        }
        ids
    }

    pub fn is_empty(&self) -> bool {
        self.tmdb
            .is_empty()
            && self
                .imdb
                .is_empty()
            && self
                .tvdb
                .is_empty()
    }

    fn push_token(&mut self, token: &str) {
        let token = token.trim();
        if token.is_empty() {
            return;
        }
        let Some((provider, value)) = token
            .split_once('.')
            .or_else(|| token.split_once(':'))
        else {
            return;
        };
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let Ok(kind) = ProviderAlias::from_str(provider.trim()) else {
            return;
        };
        match kind {
            ProviderAlias::Tmdb => {
                if let Ok(n) = value.parse::<i64>() {
                    if n > 0
                        && !self
                            .tmdb
                            .contains(&n)
                    {
                        self.tmdb
                            .push(n);
                    }
                }
            }
            ProviderAlias::Imdb => {
                if !self
                    .imdb
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(value))
                {
                    self.imdb
                        .push(value.to_string());
                }
            }
            ProviderAlias::Tvdb => {
                if let Ok(n) = value.parse::<i64>() {
                    if n > 0
                        && !self
                            .tvdb
                            .contains(&n)
                    {
                        self.tvdb
                            .push(n);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_emby_style_tmdb_imdb_tvdb_tokens() {
        let ids = AnyProviderIds::parse(&[
            "Tmdb.27205".to_string(),
            "Imdb.tt1375666".to_string(),
            "Tvdb.81189".to_string(),
        ]);
        assert_eq!(
            ids,
            AnyProviderIds {
                tmdb: vec![27205],
                imdb: vec!["tt1375666".to_string()],
                tvdb: vec![81189],
            }
        );
    }

    #[test]
    fn accepts_colon_separator_and_provider_aliases() {
        let ids = AnyProviderIds::parse(&[
            "themoviedb:550".to_string(),
            "imdbid.tt0137523".to_string(),
            "TheTvdb.81189".to_string(),
        ]);
        assert_eq!(ids.tmdb, vec![550]);
        assert_eq!(ids.imdb, vec!["tt0137523".to_string()]);
        assert_eq!(ids.tvdb, vec![81189]);
    }

    #[test]
    fn ignores_unknown_providers_and_empty_values() {
        let ids = AnyProviderIds::parse(&[
            "Tmdb.".to_string(),
            "Kitsu.123".to_string(),
            "not-a-provider-id".to_string(),
            "".to_string(),
        ]);
        assert!(ids.is_empty());
    }

    #[test]
    fn deduplicates_repeated_tokens() {
        let ids = AnyProviderIds::parse(&[
            "Tmdb.550".to_string(),
            "tmdb:550".to_string(),
            "Imdb.tt0137523".to_string(),
            "imdb.tt0137523".to_string(),
        ]);
        assert_eq!(ids.tmdb, vec![550]);
        assert_eq!(ids.imdb, vec!["tt0137523".to_string()]);
    }
}
