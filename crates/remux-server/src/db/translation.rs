use std::{collections::HashMap, sync::Arc};

use remux_sdks::remux::MetadataLanguage;
use sqlx::SqlitePool;
use uuid::Uuid;

use super::{ImageKind, Media, MediaImage, MetadataField};

/// Where a `media_translations` row came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum TranslationSource {
    Provider,
    Manual,
}

/// Title/description and Primary image URL for one media item in one
/// non-default language.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaTranslation {
    pub media_id: Uuid,
    pub language: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub primary_image: Option<String>,
    pub source: TranslationSource,
    #[sqlx(json(nullable))]
    pub locked_fields: Option<Vec<MetadataField>>,
}

/// Provider text for one media item in one non-default language.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslatedText {
    pub language: MetadataLanguage,
    pub title: Option<String>,
    pub description: Option<String>,
    pub primary_image: Option<String>,
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
}

impl MediaTranslation {
    /// Translation per media id for `language`, taking each field from the
    /// full tag's row before its primary subtag's (`pt-br`, then `pt`). The
    /// result carries the most specific row's language and locks.
    pub async fn get_for_media_ids(
        db: &SqlitePool,
        media_ids: &[Uuid],
        language: &MetadataLanguage,
    ) -> Result<HashMap<Uuid, Self>, sqlx::Error> {
        Ok(Self::rows_for_media_ids(db, media_ids, language)
            .await?
            .into_iter()
            .map(|(id, rows)| (id, Self::merge_fallbacks(rows)))
            .collect())
    }

    /// Stored rows per media id in `language`'s fallback tags, most
    /// specific first.
    async fn rows_for_media_ids(
        db: &SqlitePool,
        media_ids: &[Uuid],
        language: &MetadataLanguage,
    ) -> Result<HashMap<Uuid, Vec<Self>>, sqlx::Error> {
        let mut out: HashMap<Uuid, Vec<Self>> = HashMap::new();
        if media_ids.is_empty() {
            return Ok(out);
        }
        let fallbacks = language.fallbacks();
        // SQLite's default bound-parameter limit is 32766; stay far below it.
        for chunk in media_ids.chunks(900) {
            let mut qb = sqlx::QueryBuilder::new(
                "SELECT media_id, language, title, description, primary_image, source, \
                 locked_fields FROM media_translations WHERE language IN (",
            );
            let mut sep = qb.separated(", ");
            for tag in &fallbacks {
                sep.push_bind(*tag);
            }
            qb.push(") AND media_id IN (");
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            qb.push(")");
            let rows = qb
                .build_query_as::<Self>()
                .fetch_all(db)
                .await?;
            for row in rows {
                out.entry(row.media_id)
                    .or_default()
                    .push(row);
            }
        }
        let rank = |t: &Self| {
            fallbacks
                .iter()
                .position(|f| *f == t.language)
                .unwrap_or(usize::MAX)
        };
        for rows in out.values_mut() {
            rows.sort_by_key(|t| rank(t));
        }
        Ok(out)
    }

    /// Fill the first row's empty fields from the rows after it.
    fn merge_fallbacks(rows: Vec<Self>) -> Self {
        let mut rows = rows.into_iter();
        let mut merged = rows
            .next()
            .expect("rows_for_media_ids never yields an empty list");
        for row in rows {
            for (field, fallback) in [
                (&mut merged.title, row.title),
                (&mut merged.description, row.description),
                (&mut merged.primary_image, row.primary_image),
            ] {
                if non_empty(field.as_deref()).is_none() {
                    *field = fallback;
                }
            }
        }
        merged
    }

    /// Upsert provider text. Fields listed in an existing row's
    /// `locked_fields` keep their stored value, rows for item-locked or
    /// missing media are skipped, and a manual row stays marked manual.
    pub async fn upsert_provider(
        db: &SqlitePool,
        rows: &[(Uuid, TranslatedText)],
    ) -> Result<(), sqlx::Error> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut tx = db
            .begin()
            .await?;
        for (media_id, row) in rows {
            let title = non_empty(
                row.title
                    .as_deref(),
            );
            let description = non_empty(
                row.description
                    .as_deref(),
            );
            let primary_image = non_empty(
                row.primary_image
                    .as_deref(),
            );
            if title.is_none() && description.is_none() && primary_image.is_none() {
                continue;
            }
            sqlx::query(
                "INSERT INTO media_translations \
                   (media_id, language, kind, title, description, primary_image, source, updated_at) \
                 SELECT id, ?2, kind, ?3, ?4, ?5, 'provider', CURRENT_TIMESTAMP \
                 FROM media WHERE id = ?1 AND NOT is_locked \
                 ON CONFLICT (media_id, language) DO UPDATE SET \
                   title = CASE WHEN EXISTS (SELECT 1 FROM json_each(COALESCE(media_translations.locked_fields, '[]')) WHERE value = 'Name') \
                                OR excluded.title IS NULL \
                           THEN media_translations.title ELSE excluded.title END, \
                   description = CASE WHEN EXISTS (SELECT 1 FROM json_each(COALESCE(media_translations.locked_fields, '[]')) WHERE value = 'Overview') \
                                      OR excluded.description IS NULL \
                                 THEN media_translations.description ELSE excluded.description END, \
                   primary_image = COALESCE(excluded.primary_image, media_translations.primary_image), \
                   updated_at = excluded.updated_at",
            )
            .bind(media_id)
            .bind(
                row.language
                    .as_str(),
            )
            .bind(title)
            .bind(description)
            .bind(primary_image)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
    }
}

/// SQL expression for the title a reader of `language` sees for the media
/// row aliased `alias`: their translation when one exists, else
/// `{alias}.title`. `MetadataLanguage` only holds `[a-z0-9-]`, so inlining it
/// as a literal is safe; ORDER BY clauses are assembled as plain strings and
/// can't carry bind parameters.
pub fn display_title_sql(alias: &str, language: &MetadataLanguage) -> String {
    format!(
        "COALESCE((SELECT mt.title FROM media_translations mt \
          WHERE mt.media_id = {alias}.id AND mt.language IN ({}) AND mt.title <> '' \
          ORDER BY length(mt.language) DESC LIMIT 1), {alias}.title)",
        language_list_sql(language)
    )
}

/// `'pt-br', 'pt'` — see [`display_title_sql`] for why this is inlined.
pub fn language_list_sql(language: &MetadataLanguage) -> String {
    language
        .fallbacks()
        .iter()
        .map(|tag| format!("'{tag}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Image id, and so the `ImageTags` value, of a translated Primary image.
/// The image endpoint maps it back through [`MediaTranslation::primary_image_for_tag`].
pub fn translated_image_id(media_id: Uuid, url: &str) -> Uuid {
    crate::common::get_stable_uuid(format!("translated-image:{media_id}:{url}"))
}

impl MediaTranslation {
    /// URL of the translated Primary image of `media_id` whose tag is `tag`.
    pub async fn primary_image_for_tag(
        db: &SqlitePool,
        media_id: Uuid,
        tag: Uuid,
    ) -> Result<Option<String>, sqlx::Error> {
        let urls: Vec<String> = sqlx::query_scalar(
            "SELECT primary_image FROM media_translations \
             WHERE media_id = ? AND primary_image IS NOT NULL",
        )
        .bind(media_id)
        .fetch_all(db)
        .await?;
        Ok(urls
            .into_iter()
            .find(|url| translated_image_id(media_id, url) == tag))
    }
}

impl Media {
    /// Overlay translated text and Primary image. Empty translated fields
    /// leave the server-language values in place.
    pub fn apply_translation(&mut self, translation: &MediaTranslation) {
        if let Some(title) = non_empty(
            translation
                .title
                .as_deref(),
        ) {
            self.title = title.to_string();
        }
        if let Some(description) = non_empty(
            translation
                .description
                .as_deref(),
        ) {
            self.description = Some(description.to_string());
        }
        if let Some(url) = non_empty(
            translation
                .primary_image
                .as_deref(),
        ) {
            let primary = &mut self
                .images
                .primary;
            primary.retain(|i| i.image_index != 0);
            primary.push(MediaImage {
                id: translated_image_id(self.id, url),
                media_id: self.id,
                image_type: ImageKind::Primary.to_string(),
                image_index: 0,
                path: url.to_string(),
                width: None,
                height: None,
            });
        }
    }

    /// Overlay `language` onto each row plus its loaded parent, grandparent
    /// and relation rows (genres, studios, …), with one query per call.
    /// Lookup failures leave the rows unchanged; they never fail the request.
    pub async fn resolve_translations(
        db: &SqlitePool,
        media: &mut [Media],
        language: Option<&MetadataLanguage>,
    ) {
        let Some(language) = language else {
            return;
        };
        let mut ids: Vec<Uuid> = Vec::new();
        for m in media.iter() {
            collect_translatable_ids(m, &mut ids);
        }
        ids.sort_unstable();
        ids.dedup();
        let by_id = match MediaTranslation::get_for_media_ids(db, &ids, language).await
        {
            Ok(by_id) => by_id,
            Err(e) => {
                tracing::warn!(error = %e, "loading media translations failed");
                return;
            }
        };
        for m in media.iter_mut() {
            apply_translations_deep(m, &by_id);
        }
    }

    /// Single-item form of [`Self::resolve_translations`].
    pub async fn resolve_translation(
        &mut self,
        db: &SqlitePool,
        language: Option<&MetadataLanguage>,
    ) {
        Self::resolve_translations(db, std::slice::from_mut(self), language).await;
    }
}

fn collect_translatable_ids(m: &Media, ids: &mut Vec<Uuid>) {
    ids.push(m.id);
    if let Some(p) = &m.parent {
        ids.push(p.id);
    }
    if let Some(gp) = &m.grandparent {
        ids.push(gp.id);
    }
    if let Some(rels) = &m.relations {
        ids.extend(
            rels.iter()
                .map(|(_, r)| r.id),
        );
    }
}

fn apply_translations_deep(m: &mut Media, by_id: &HashMap<Uuid, MediaTranslation>) {
    if let Some(t) = by_id.get(&m.id) {
        m.apply_translation(t);
    }
    for slot in [&mut m.parent, &mut m.grandparent] {
        if let Some(arc) = slot {
            if let Some(t) = by_id.get(&arc.id) {
                Arc::make_mut(arc).apply_translation(t);
            }
        }
    }
    if let Some(rels) = &mut m.relations {
        for (_, r) in rels.iter_mut() {
            if let Some(t) = by_id.get(&r.id) {
                r.apply_translation(t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{MediaKind, MediaRelation};

    fn lang(s: &str) -> MetadataLanguage {
        s.parse()
            .unwrap()
    }

    fn translation(
        media_id: Uuid,
        title: Option<&str>,
        description: Option<&str>,
    ) -> MediaTranslation {
        MediaTranslation {
            media_id,
            language: "es".to_string(),
            title: title.map(str::to_string),
            description: description.map(str::to_string),
            primary_image: None,
            source: TranslationSource::Provider,
            locked_fields: None,
        }
    }

    fn movie(title: &str) -> Media {
        Media {
            id: Uuid::new_v4(),
            title: title.to_string(),
            description: Some(format!("{title} description")),
            kind: MediaKind::Movie,
            ..Default::default()
        }
    }

    #[test]
    fn apply_translation_overlays_title_and_description() {
        let mut m = movie("The Matrix");
        m.apply_translation(&translation(m.id, Some("Matrix"), Some("Descripción")));
        assert_eq!(m.title, "Matrix");
        assert_eq!(
            m.description
                .as_deref(),
            Some("Descripción")
        );
    }

    #[test]
    fn apply_translation_keeps_fallback_for_empty_fields() {
        let mut m = movie("The Matrix");
        m.apply_translation(&translation(m.id, Some("  "), None));
        assert_eq!(m.title, "The Matrix");
        assert_eq!(
            m.description
                .as_deref(),
            Some("The Matrix description")
        );
    }

    #[test]
    fn apply_translation_shows_locked_translations() {
        let mut m = movie("The Matrix");
        m.is_locked = true;
        let mut t = translation(m.id, Some("Matrix"), None);
        t.locked_fields = Some(vec![MetadataField::Name]);
        m.apply_translation(&t);
        assert_eq!(m.title, "Matrix", "locks gate provider writes, not display");
    }

    #[test]
    fn apply_translation_replaces_primary_image_with_a_tagged_one() {
        let mut m = movie("The Matrix");
        m.set_image(ImageKind::Primary, "https://img/server.jpg".into());
        let server_id = m
            .images
            .get(ImageKind::Primary)
            .unwrap()
            .id;
        let mut t = translation(m.id, None, None);
        t.primary_image = Some("https://img/es.jpg".into());
        m.apply_translation(&t);
        let primary = m
            .images
            .get(ImageKind::Primary)
            .unwrap();
        assert_eq!(primary.path, "https://img/es.jpg");
        assert_eq!(primary.id, translated_image_id(m.id, "https://img/es.jpg"));
        assert_ne!(primary.id, server_id);
        assert_eq!(
            m.images
                .primary
                .len(),
            1
        );
    }

    #[test]
    fn deep_apply_translates_parents_and_relations() {
        let series = movie("Breaking Bad");
        let season = movie("Season 1");
        let genre = Media {
            id: Uuid::new_v4(),
            title: "Drama".to_string(),
            kind: MediaKind::Genre,
            ..Default::default()
        };
        let mut episode = movie("Pilot");
        episode.parent = Some(Arc::new(season.clone()));
        episode.grandparent = Some(Arc::new(series.clone()));
        episode.relations = Some(vec![(MediaRelation::default(), genre.clone())]);

        let mut ids = Vec::new();
        collect_translatable_ids(&episode, &mut ids);
        assert_eq!(ids, vec![episode.id, season.id, series.id, genre.id]);

        let by_id: HashMap<Uuid, MediaTranslation> = [
            (episode.id, translation(episode.id, Some("Piloto"), None)),
            (season.id, translation(season.id, Some("Temporada 1"), None)),
            (
                series.id,
                translation(series.id, Some("Breaking Bad ES"), None),
            ),
            (genre.id, translation(genre.id, Some("Drama ES"), None)),
        ]
        .into_iter()
        .collect();
        apply_translations_deep(&mut episode, &by_id);
        assert_eq!(episode.title, "Piloto");
        assert_eq!(
            episode
                .parent
                .as_ref()
                .unwrap()
                .title,
            "Temporada 1"
        );
        assert_eq!(
            episode
                .grandparent
                .as_ref()
                .unwrap()
                .title,
            "Breaking Bad ES"
        );
        assert_eq!(
            episode
                .relations
                .as_ref()
                .unwrap()[0]
                .1
                .title,
            "Drama ES"
        );
        assert_eq!(
            season.title, "Season 1",
            "shared Arc parents are copied, not mutated"
        );
    }

    #[test]
    fn display_title_sql_inlines_fallback_tags() {
        let sql = display_title_sql("g", &lang("pt-BR"));
        assert!(sql.contains("IN ('pt-br', 'pt')"), "{sql}");
        assert!(sql.contains("mt.media_id = g.id"), "{sql}");
        assert!(sql.ends_with("g.title)"), "{sql}");
    }

    async fn test_db() -> SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 10_000)
            .await
            .unwrap();
        crate::db::migrate(&db)
            .await
            .unwrap();
        db
    }

    async fn insert_movie(db: &SqlitePool, title: &str) -> Media {
        let m = movie(title);
        sqlx::query(
            "INSERT INTO media (id, title, kind, external_ids, locked_fields, created_at, updated_at) \
             VALUES (?, ?, 'movie', '{}', '[]', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(m.id)
        .bind(&m.title)
        .execute(db)
        .await
        .unwrap();
        m
    }

    #[tokio::test]
    async fn primary_image_for_tag_finds_the_tagged_image() {
        let db = test_db().await;
        let m = insert_movie(&db, "The Matrix").await;
        MediaTranslation::upsert_provider(
            &db,
            &[(
                m.id,
                TranslatedText {
                    language: lang("es"),
                    title: None,
                    description: None,
                    primary_image: Some("https://img/es.jpg".into()),
                },
            )],
        )
        .await
        .unwrap();
        let tag = translated_image_id(m.id, "https://img/es.jpg");
        assert_eq!(
            MediaTranslation::primary_image_for_tag(&db, m.id, tag)
                .await
                .unwrap()
                .as_deref(),
            Some("https://img/es.jpg")
        );
        assert_eq!(
            MediaTranslation::primary_image_for_tag(&db, m.id, Uuid::new_v4())
                .await
                .unwrap(),
            None
        );

        MediaTranslation::upsert_provider(
            &db,
            &[(
                m.id,
                TranslatedText {
                    language: lang("es"),
                    title: Some("Matrix".into()),
                    description: None,
                    primary_image: None,
                },
            )],
        )
        .await
        .unwrap();
        assert_eq!(
            MediaTranslation::primary_image_for_tag(&db, m.id, tag)
                .await
                .unwrap()
                .as_deref(),
            Some("https://img/es.jpg"),
            "a refresh without a poster keeps the stored one"
        );
    }

    #[tokio::test]
    async fn provider_upsert_and_fallback_lookup() {
        let db = test_db().await;
        let m = insert_movie(&db, "The Matrix").await;
        MediaTranslation::upsert_provider(
            &db,
            &[(
                m.id,
                TranslatedText {
                    language: lang("pt"),
                    title: Some("Matrix".into()),
                    description: Some("Um hacker…".into()),
                    primary_image: None,
                },
            )],
        )
        .await
        .unwrap();

        let found = MediaTranslation::get_for_media_ids(&db, &[m.id], &lang("pt-br"))
            .await
            .unwrap();
        assert_eq!(
            found[&m.id]
                .title
                .as_deref(),
            Some("Matrix")
        );

        MediaTranslation::upsert_provider(
            &db,
            &[(
                m.id,
                TranslatedText {
                    language: lang("pt-br"),
                    title: Some("Matrix BR".into()),
                    description: None,
                    primary_image: None,
                },
            )],
        )
        .await
        .unwrap();
        let found = MediaTranslation::get_for_media_ids(&db, &[m.id], &lang("pt-br"))
            .await
            .unwrap();
        assert_eq!(
            found[&m.id]
                .title
                .as_deref(),
            Some("Matrix BR"),
            "full tag wins over base"
        );
        assert_eq!(
            found[&m.id]
                .description
                .as_deref(),
            Some("Um hacker…"),
            "fields missing from the full tag come from the base"
        );
        assert_eq!(found[&m.id].language, "pt-br");
    }

    async fn seed_translated_movies(db: &SqlitePool) -> Vec<Media> {
        // server-language title -> Spanish title
        let pairs = [
            ("The Matrix", "Matrix"),
            ("Spirited Away", "El viaje de Chihiro"),
            ("Up", "Up: Una aventura de altura"),
        ];
        let mut out = Vec::new();
        for (en, es) in pairs {
            let m = insert_movie(db, en).await;
            MediaTranslation::upsert_provider(
                db,
                &[(
                    m.id,
                    TranslatedText {
                        language: lang("es"),
                        title: Some(es.to_string()),
                        description: None,
                        primary_image: None,
                    },
                )],
            )
            .await
            .unwrap();
            out.push(m);
        }
        out
    }

    async fn titles(db: &SqlitePool, filter: crate::db::MediaFilter) -> Vec<String> {
        Media::get_by_filter(db, &filter)
            .await
            .unwrap()
            .records
            .into_iter()
            .map(|m| m.title)
            .collect()
    }

    fn movie_filter(language: Option<&str>) -> crate::db::MediaFilter {
        crate::db::MediaFilter {
            kind: Some(vec![MediaKind::Movie]),
            sort_by: vec![crate::api::ItemSortBy::SortName],
            sort_order: vec![crate::api::SortOrder::Ascending],
            metadata_language: language.map(lang),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn search_matches_translated_and_server_titles() {
        let db = test_db().await;
        seed_translated_movies(&db).await;

        let mut f = movie_filter(Some("es"));
        f.title_contains = Some("chihiro".into());
        assert_eq!(titles(&db, f).await, vec!["El viaje de Chihiro"]);

        let mut f = movie_filter(Some("es"));
        f.title_contains = Some("spirited".into());
        assert_eq!(
            titles(&db, f).await,
            vec!["El viaje de Chihiro"],
            "server-language title still matches, shown translated"
        );

        let mut f = movie_filter(None);
        f.title_contains = Some("chihiro".into());
        assert!(
            titles(&db, f)
                .await
                .is_empty(),
            "default-language readers are unaffected"
        );
    }

    #[tokio::test]
    async fn name_sort_and_jump_bar_use_translated_titles() {
        let db = test_db().await;
        seed_translated_movies(&db).await;

        assert_eq!(
            titles(&db, movie_filter(None)).await,
            vec!["Spirited Away", "The Matrix", "Up"]
        );
        assert_eq!(
            titles(&db, movie_filter(Some("es"))).await,
            vec![
                "El viaje de Chihiro",
                "Matrix",
                "Up: Una aventura de altura"
            ]
        );

        let mut f = movie_filter(Some("es"));
        f.sort_by = vec![crate::api::ItemSortBy::Default];
        assert_eq!(
            titles(&db, f).await,
            vec![
                "El viaje de Chihiro",
                "Matrix",
                "Up: Una aventura de altura"
            ]
        );

        let mut f = movie_filter(Some("es"));
        f.name_starts_with = Some("M".into());
        assert_eq!(titles(&db, f).await, vec!["Matrix"]);

        let mut f = movie_filter(Some("es"));
        f.name_less_than = Some("M".into());
        f.total_count = true;
        let res = Media::get_by_filter(&db, &f)
            .await
            .unwrap();
        assert_eq!(
            res.total_count, 1,
            "count query uses the same title expression"
        );
    }

    #[tokio::test]
    async fn regional_reader_sorts_by_base_title_when_regional_row_has_none() {
        let db = test_db().await;
        let movies = seed_translated_movies(&db).await;
        MediaTranslation::upsert_provider(
            &db,
            &[(
                movies[0].id,
                TranslatedText {
                    language: lang("es-mx"),
                    title: None,
                    description: Some("Un hacker…".into()),
                    primary_image: None,
                },
            )],
        )
        .await
        .unwrap();
        assert_eq!(
            titles(&db, movie_filter(Some("es-mx"))).await,
            vec![
                "El viaje de Chihiro",
                "Matrix",
                "Up: Una aventura de altura"
            ]
        );
        let mut f = movie_filter(Some("es-mx"));
        f.name_starts_with = Some("M".into());
        assert_eq!(titles(&db, f).await, vec!["Matrix"]);
    }
}
