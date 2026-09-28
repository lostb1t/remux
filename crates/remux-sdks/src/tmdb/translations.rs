use serde::{Deserialize, Serialize};

use crate::{Endpoint, remux::MetadataLanguage};

/// `append_to_response=translations` payload for movies and series.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Translations {
    #[serde(default)]
    pub translations: Vec<Translation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Translation {
    pub iso_639_1: String,
    pub iso_3166_1: String,
    #[serde(default)]
    pub data: TranslationData,
}

/// Movies carry `title`, series carry `name`; either may be empty when the
/// translated title equals the original.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranslationData {
    pub title: Option<String>,
    pub name: Option<String>,
    pub overview: Option<String>,
}

fn non_empty(s: Option<&String>) -> Option<String> {
    s.map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Home region for a bare language: its code uppercased (`es` → `ES`),
/// except where TMDB's main region uses another code (`en` → `US`).
fn default_region(base: &str) -> String {
    match base {
        "en" => "US",
        "zh" => "CN",
        "ar" => "SA",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

impl Translations {
    /// Title and overview for `language`. A region-tagged language
    /// (`pt-br`) wants that exact region; a bare one (`es`) prefers the
    /// language's home region, then any region. Empty fields are filled from
    /// the other regions of the same language.
    pub fn text_for(
        &self,
        language: &MetadataLanguage,
    ) -> (Option<String>, Option<String>) {
        let base = language.base();
        let wanted_region = if base == language.as_str() {
            default_region(base)
        } else {
            language.as_str()[base.len() + 1..].to_ascii_uppercase()
        };
        let mut same_language: Vec<&Translation> = self
            .translations
            .iter()
            .filter(|t| {
                t.iso_639_1
                    .eq_ignore_ascii_case(base)
            })
            .collect();
        if same_language.is_empty() {
            return (None, None);
        }
        let region_tagged = base != language.as_str();
        if region_tagged
            && !same_language
                .iter()
                .any(|t| {
                    t.iso_3166_1
                        .eq_ignore_ascii_case(&wanted_region)
                })
        {
            return (None, None);
        }
        same_language.sort_by_key(|t| {
            !t.iso_3166_1
                .eq_ignore_ascii_case(&wanted_region)
        });
        let title = if region_tagged {
            non_empty(
                same_language[0]
                    .data
                    .title
                    .as_ref(),
            )
            .or_else(|| {
                non_empty(
                    same_language[0]
                        .data
                        .name
                        .as_ref(),
                )
            })
        } else {
            same_language
                .iter()
                .find_map(|t| {
                    non_empty(
                        t.data
                            .title
                            .as_ref(),
                    )
                    .or_else(|| {
                        non_empty(
                            t.data
                                .name
                                .as_ref(),
                        )
                    })
                })
        };
        let overview = if region_tagged {
            non_empty(
                same_language[0]
                    .data
                    .overview
                    .as_ref(),
            )
        } else {
            same_language
                .iter()
                .find_map(|t| {
                    non_empty(
                        t.data
                            .overview
                            .as_ref(),
                    )
                })
        };
        (title, overview)
    }
}

/// TMDB's `language` query parameter for a metadata language: `es`, `pt-BR`.
pub fn language_param(language: &MetadataLanguage) -> String {
    let base = language.base();
    if base == language.as_str() {
        base.to_string()
    } else {
        format!(
            "{base}-{}",
            language.as_str()[base.len() + 1..].to_ascii_uppercase()
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GenreListKind {
    #[default]
    Movie,
    Tv,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreListEndpoint {
    #[serde(skip)]
    pub kind: GenreListKind,
    pub language: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenreList {
    #[serde(default)]
    pub genres: Vec<super::Genre>,
}

impl Endpoint for GenreListEndpoint {
    type Output = GenreList;

    fn path(&self) -> String {
        match self.kind {
            GenreListKind::Movie => "genre/movie/list".to_string(),
            GenreListKind::Tv => "genre/tv/list".to_string(),
        }
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(s: &str) -> MetadataLanguage {
        s.parse()
            .unwrap()
    }

    fn tr(lang: &str, region: &str, title: &str, overview: &str) -> Translation {
        Translation {
            iso_639_1: lang.into(),
            iso_3166_1: region.into(),
            data: TranslationData {
                title: Some(title.into()),
                name: None,
                overview: Some(overview.into()),
            },
        }
    }

    fn sample() -> Translations {
        Translations {
            translations: vec![
                tr("es", "MX", "El viaje de Chihiro (MX)", "MX overview"),
                tr("es", "ES", "El viaje de Chihiro", "ES overview"),
                tr("pt", "BR", "A Viagem de Chihiro", "BR overview"),
                tr("ja", "JP", "千と千尋の神隠し", ""),
            ],
        }
    }

    #[test]
    fn bare_language_prefers_home_region() {
        assert_eq!(
            sample().text_for(&lang("es")),
            (
                Some("El viaje de Chihiro".into()),
                Some("ES overview".into())
            )
        );
    }

    #[test]
    fn bare_english_prefers_us() {
        let translations = Translations {
            translations: vec![
                tr("en", "GB", "Spirited Away (GB)", "GB overview"),
                tr("en", "US", "Spirited Away", "US overview"),
            ],
        };
        assert_eq!(
            translations.text_for(&lang("en")),
            (Some("Spirited Away".into()), Some("US overview".into()))
        );
    }

    #[test]
    fn bare_language_falls_back_to_any_region() {
        assert_eq!(
            sample().text_for(&lang("pt")),
            (
                Some("A Viagem de Chihiro".into()),
                Some("BR overview".into())
            )
        );
    }

    #[test]
    fn region_tagged_language_wants_that_region() {
        assert_eq!(
            sample().text_for(&lang("es-mx")),
            (
                Some("El viaje de Chihiro (MX)".into()),
                Some("MX overview".into())
            )
        );
        assert_eq!(sample().text_for(&lang("pt-pt")), (None, None));
    }

    #[test]
    fn empty_fields_come_back_as_none() {
        assert_eq!(
            sample().text_for(&lang("ja")),
            (Some("千と千尋の神隠し".into()), None)
        );
        assert_eq!(sample().text_for(&lang("fr")), (None, None));
    }

    #[test]
    fn series_name_is_used_as_title() {
        let t = Translations {
            translations: vec![Translation {
                iso_639_1: "fr".into(),
                iso_3166_1: "FR".into(),
                data: TranslationData {
                    title: None,
                    name: Some("Le Bureau".into()),
                    overview: None,
                },
            }],
        };
        assert_eq!(
            t.text_for(&lang("fr"))
                .0
                .as_deref(),
            Some("Le Bureau")
        );
    }

    #[test]
    fn language_param_uppercases_region() {
        assert_eq!(language_param(&lang("pt-br")), "pt-BR");
        assert_eq!(language_param(&lang("es")), "es");
    }
}
