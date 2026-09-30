use std::{fmt, str::FromStr};

/// A normalized metadata language tag: a 2-3 letter ISO 639 code with an
/// optional region/script subtag, lowercased (`"es"`, `"pt-br"`, `"zh-hant"`).
/// `_` is accepted as a separator and normalized to `-`.
///
/// Only ASCII letters, digits and a single `-` can appear in the inner
/// string, so it is safe to embed as a SQL string literal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MetadataLanguage(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid metadata language tag: {0:?}")]
pub struct InvalidMetadataLanguage(pub String);

impl MetadataLanguage {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The primary language subtag: `"pt"` for `"pt-br"`, itself otherwise.
    pub fn base(&self) -> &str {
        self.0
            .split_once('-')
            .map_or(
                self.0
                    .as_str(),
                |(base, _)| base,
            )
    }

    /// Tags to try in order when looking up a translation: the full tag,
    /// then the primary language subtag when there is a region/script.
    pub fn fallbacks(&self) -> Vec<&str> {
        let base = self.base();
        if base == self.0 {
            vec![
                self.0
                    .as_str(),
            ]
        } else {
            vec![
                self.0
                    .as_str(),
                base,
            ]
        }
    }

    /// Whether text stored in `default` already serves a reader of this
    /// language: the same tag, or a bare language (`en`) whose regional
    /// variant (`en-us`) is the default.
    pub fn is_covered_by(&self, default: &MetadataLanguage) -> bool {
        self == default
            || (self.base() == self.as_str() && self.base() == default.base())
    }

    /// Parse an optional preference, treating `None`, empty strings and
    /// unparseable tags as unset.
    pub fn parse_pref(pref: Option<&str>) -> Option<Self> {
        pref.and_then(|s| {
            s.parse()
                .ok()
        })
    }
}

impl FromStr for MetadataLanguage {
    type Err = InvalidMetadataLanguage;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s
            .trim()
            .to_ascii_lowercase()
            .replace('_', "-");
        let mut parts = normalized.split('-');
        let base_ok = parts
            .next()
            .is_some_and(|b| {
                (2..=3).contains(&b.len())
                    && b.bytes()
                        .all(|c| c.is_ascii_lowercase())
            });
        let rest: Vec<&str> = parts.collect();
        let rest_ok = match rest.as_slice() {
            [] => true,
            [sub] => {
                (2..=8).contains(&sub.len())
                    && sub
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric())
            }
            _ => false,
        };
        if base_ok && rest_ok {
            Ok(Self(normalized))
        } else {
            Err(InvalidMetadataLanguage(s.to_string()))
        }
    }
}

impl fmt::Display for MetadataLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(s: &str) -> MetadataLanguage {
        s.parse()
            .unwrap()
    }

    #[test]
    fn bare_language_is_covered_by_its_regional_default() {
        assert!(lang("en").is_covered_by(&lang("en-US")));
        assert!(lang("en-us").is_covered_by(&lang("en-US")));
        assert!(!lang("en-gb").is_covered_by(&lang("en-US")));
        assert!(!lang("en-us").is_covered_by(&lang("en")));
        assert!(!lang("es").is_covered_by(&lang("en")));
    }

    #[test]
    fn parses_and_normalizes() {
        assert_eq!(lang("es").as_str(), "es");
        assert_eq!(lang("pt-BR").as_str(), "pt-br");
        assert_eq!(lang(" pt_BR ").as_str(), "pt-br");
        assert_eq!(lang("zh-Hant").as_str(), "zh-hant");
        assert_eq!(lang("fil").as_str(), "fil");
    }

    #[test]
    fn rejects_malformed_tags() {
        for bad in [
            "", " ", "e", "english", "es-", "-es", "es-mx-x", "es'", "es;--", "e1",
        ] {
            assert!(
                bad.parse::<MetadataLanguage>()
                    .is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn fallbacks_include_base_for_regional_tags() {
        assert_eq!(lang("pt-br").fallbacks(), vec!["pt-br", "pt"]);
        assert_eq!(lang("es").fallbacks(), vec!["es"]);
    }

    #[test]
    fn parse_pref_treats_empty_and_invalid_as_unset() {
        assert_eq!(MetadataLanguage::parse_pref(None), None);
        assert_eq!(MetadataLanguage::parse_pref(Some("")), None);
        assert_eq!(MetadataLanguage::parse_pref(Some("??")), None);
        assert_eq!(MetadataLanguage::parse_pref(Some("fr")), Some(lang("fr")));
    }
}
