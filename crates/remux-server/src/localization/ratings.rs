use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::api::{ParentalRating, ParentalRatingScore};
use crate::db::normalize_country_alpha2;

const RATING_FILES: &[&str] = &[
    include_str!("ratings/0-prefer.json"),
    include_str!("ratings/ar.json"),
    include_str!("ratings/au.json"),
    include_str!("ratings/be.json"),
    include_str!("ratings/bg.json"),
    include_str!("ratings/br.json"),
    include_str!("ratings/ca.json"),
    include_str!("ratings/cl.json"),
    include_str!("ratings/co.json"),
    include_str!("ratings/cz.json"),
    include_str!("ratings/de.json"),
    include_str!("ratings/dk.json"),
    include_str!("ratings/es.json"),
    include_str!("ratings/fi.json"),
    include_str!("ratings/fr.json"),
    include_str!("ratings/gb.json"),
    include_str!("ratings/gr.json"),
    include_str!("ratings/hu.json"),
    include_str!("ratings/id.json"),
    include_str!("ratings/ie.json"),
    include_str!("ratings/in.json"),
    include_str!("ratings/it.json"),
    include_str!("ratings/jp.json"),
    include_str!("ratings/kr.json"),
    include_str!("ratings/kz.json"),
    include_str!("ratings/lt.json"),
    include_str!("ratings/mx.json"),
    include_str!("ratings/nl.json"),
    include_str!("ratings/no.json"),
    include_str!("ratings/nz.json"),
    include_str!("ratings/ph.json"),
    include_str!("ratings/pl.json"),
    include_str!("ratings/pt.json"),
    include_str!("ratings/ro.json"),
    include_str!("ratings/ru.json"),
    include_str!("ratings/se.json"),
    include_str!("ratings/sg.json"),
    include_str!("ratings/sk.json"),
    include_str!("ratings/th.json"),
    include_str!("ratings/tr.json"),
    include_str!("ratings/tw.json"),
    include_str!("ratings/ua.json"),
    include_str!("ratings/uk.json"),
    include_str!("ratings/us.json"),
    include_str!("ratings/za.json"),
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RatingSystem {
    country_code: String,
    ratings: Option<Vec<RatingEntry>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RatingEntry {
    rating_strings: Vec<String>,
    rating_score: ParentalRatingScore,
}

#[derive(Debug)]
struct RatingsData {
    by_country: HashMap<String, Vec<(String, ParentalRatingScore)>>,
    lookup: HashMap<String, HashMap<String, ParentalRatingScore>>,
}

static RATINGS: OnceLock<RatingsData> = OnceLock::new();

fn ratings_data() -> &'static RatingsData {
    RATINGS.get_or_init(|| {
        let mut by_country = HashMap::new();
        let mut lookup = HashMap::new();

        for raw in RATING_FILES {
            let Ok(system) = serde_json::from_str::<RatingSystem>(raw) else {
                continue;
            };
            let country = normalize_country_alpha2(&system.country_code);
            let mut list = Vec::new();
            let mut country_lookup = HashMap::new();
            for entry in system
                .ratings
                .unwrap_or_default()
            {
                for rating in entry.rating_strings {
                    country_lookup.insert(
                        rating.to_lowercase(),
                        entry
                            .rating_score
                            .clone(),
                    );
                    list.push((
                        rating,
                        entry
                            .rating_score
                            .clone(),
                    ));
                }
            }
            by_country.insert(country.clone(), list);
            lookup.insert(country, country_lookup);
        }

        RatingsData { by_country, lookup }
    })
}

pub fn parental_ratings_for_country(country_code: Option<&str>) -> Vec<ParentalRating> {
    let country = country_code
        .map(normalize_country_alpha2)
        .unwrap_or_else(|| "US".to_string());
    let data = ratings_data();
    let mut ratings: Vec<ParentalRating> = data
        .by_country
        .get(&country)
        .or_else(|| {
            data.by_country
                .get("US")
        })
        .into_iter()
        .flat_map(|ratings| ratings.iter())
        .map(|(name, score)| ParentalRating {
            name: name.clone(),
            value: Some(score.score),
            rating_score: Some(score.clone()),
        })
        .collect();

    add_common_ratings(&mut ratings);
    ratings.sort_by_key(|r| {
        (
            r.rating_score
                .as_ref()
                .map(|s| s.score)
                .unwrap_or(-1),
            r.rating_score
                .as_ref()
                .and_then(|s| s.sub_score)
                .unwrap_or(0),
            r.name
                .clone(),
        )
    });
    ratings
}

fn add_common_ratings(ratings: &mut Vec<ParentalRating>) {
    if !ratings
        .iter()
        .any(|r| {
            r.rating_score
                .is_none()
        })
    {
        ratings.push(ParentalRating::unrated("Unrated"));
    }
}

pub fn resolve_rating_score(
    rating: &str,
    country_code: Option<&str>,
) -> Option<ParentalRatingScore> {
    let rating = rating.trim();
    if rating.is_empty()
        || matches!(
            rating
                .to_lowercase()
                .as_str(),
            "n/a" | "unrated" | "not rated" | "nr"
        )
    {
        return None;
    }

    if let Ok(age) = rating.parse::<i32>() {
        return Some(ParentalRatingScore {
            score: age,
            sub_score: None,
        });
    }

    let rating = rating
        .replace("Rated :", "")
        .replace("Rated:", "")
        .replace("Rated ", "")
        .trim()
        .to_string();

    let data = ratings_data();
    if let Some(country) = country_code.map(normalize_country_alpha2) {
        if let Some(score) = lookup_in_country(data, &country, &rating) {
            return Some(score);
        }
    }
    if let Some(score) = lookup_in_country(data, "US", &rating) {
        return Some(score);
    }
    for country in data
        .lookup
        .keys()
    {
        if let Some(score) = lookup_in_country(data, country, &rating) {
            return Some(score);
        }
    }
    if let Some((_, right)) = rating.rsplit_once(':') {
        return resolve_rating_score(right, None);
    }
    if let Some((left, right)) = rating.split_once('-') {
        if left.len() == 2 && !right.is_empty() {
            return resolve_rating_score(right, Some(left));
        }
    }

    None
}

fn lookup_in_country(
    data: &RatingsData,
    country: &str,
    rating: &str,
) -> Option<ParentalRatingScore> {
    data.lookup
        .get(country)
        .and_then(|ratings| ratings.get(&rating.to_lowercase()))
        .cloned()
}

pub fn resolve_rating_age(
    rating: Option<&str>,
    country_code: Option<&str>,
) -> Option<i32> {
    resolve_rating_score(rating?, country_code).map(|score| score.score)
}
