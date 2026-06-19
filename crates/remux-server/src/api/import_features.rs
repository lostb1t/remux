use crate::db::auth;
use crate::{AppState, db};
use axum::{Json, extract::State, response::IntoResponse};
use axum_anyhow::ApiResult as Result;
use csv_async::AsyncReaderBuilder;
use futures_util::StreamExt;
use remux_macros::{get, post};
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

fn normalize(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut p = false;
    for c in s
        .chars()
        .flat_map(|c| c.to_lowercase())
    {
        if c.is_alphanumeric() {
            if p && !o.is_empty() {
                o.push(' ');
            }
            o.push(c);
            p = false;
        } else {
            p = true;
        }
    }
    o
}
fn primary_artist(s: &str) -> String {
    let s = s.to_lowercase();
    let mut c = s.len();
    for sep in [
        ",",
        ";",
        " feat",
        " ft.",
        " ft ",
        " featuring",
        " & ",
        " x ",
    ] {
        if let Some(i) = s.find(sep) {
            c = c.min(i);
        }
    }
    normalize(&s[..c])
}
fn match_key(a: &str, t: &str) -> String {
    format!("{}\u{1}{}", primary_artist(a), normalize(t))
}
fn norm_feature(n: &str, v: f64) -> f64 {
    match n {
        "tempo" => (v / 250.0).clamp(0.0, 1.0),
        "loudness" => ((v + 60.0) / 60.0).clamp(0.0, 1.0),
        "popularity" => (v / 100.0).clamp(0.0, 1.0),
        _ => v.clamp(0.0, 1.0),
    }
}
fn header_role(h: &str) -> Option<&'static str> {
    match h
        .trim()
        .to_lowercase()
        .as_str()
    {
        "name" | "track_name" | "track" | "title" => Some("title"),
        "artists" | "artist_name" | "artist" | "artist_names" => Some("artist"),
        "popularity" | "track_popularity" => Some("popularity"),
        "danceability" => Some("danceability"),
        "energy" => Some("energy"),
        "valence" => Some("valence"),
        "tempo" => Some("tempo"),
        "acousticness" => Some("acousticness"),
        "instrumentalness" => Some("instrumentalness"),
        "loudness" => Some("loudness"),
        "speechiness" => Some("speechiness"),
        "liveness" => Some("liveness"),
        _ => None,
    }
}
const FEATURES: [&str; 10] = [
    "danceability",
    "energy",
    "valence",
    "tempo",
    "acousticness",
    "instrumentalness",
    "loudness",
    "speechiness",
    "liveness",
    "popularity",
];

#[derive(serde::Serialize)]
pub struct ImportStatus {
    pub scanning: bool,
    pub scanned: u64,
    pub matched: u64,
    pub total_tracks: u64,
}

pub async fn do_import(
    db: &sqlx::SqlitePool,
    urls: &str,
) -> std::result::Result<ImportStatus, anyhow::Error> {
    let url_list: Vec<&str> = urls
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if url_list.is_empty() {
        return Err(anyhow::anyhow!("No dataset URLs configured"));
    }
    let rows = sqlx::query("SELECT t.id AS id, t.title AS title, COALESCE(art.title,'') AS artist FROM media t LEFT JOIN media art ON art.id = t.grandparent_id WHERE t.kind = 'track'").fetch_all(db).await?;
    let mut lib: HashMap<String, Vec<Uuid>> = HashMap::new();
    for r in &rows {
        let (id, title, artist): (Uuid, String, String) =
            (r.get(0), r.get(1), r.get(2));
        lib.entry(match_key(&artist, &title))
            .or_default()
            .push(id);
    }
    let total = rows.len() as u64;

    let mut total_sc = 0u64;
    let mut total_mc = 0u64;
    for url in &url_list {
        let url = *url;
        tracing::info!("importing from {}", url);
        let resp = reqwest::Client::new()
            .get(url)
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await?;
        let total = resp
            .content_length()
            .unwrap_or(0);
        tracing::info!(
            "downloading {} ({:.1} MB)...",
            url.split('/')
                .last()
                .unwrap_or("dataset"),
            total as f64 / 1_048_576.0
        );
        let ext = if url
            .to_lowercase()
            .ends_with(".parquet")
        {
            "parquet"
        } else {
            "csv"
        };
        let fp = format!("/tmp/remux-import-{}.{}", Uuid::new_v4().simple(), ext);
        let body = resp
            .bytes()
            .await?;
        tokio::fs::write(&fp, &body).await?;
        let (sc, mc) = if ext == "parquet" {
            #[cfg(feature = "import-parquet")]
            {
                import_parquet(db, &fp, &lib).await?
            }
            #[cfg(not(feature = "import-parquet"))]
            {
                return Err(anyhow::anyhow!(
                    "Parquet support not compiled in — rebuild with --features import-parquet"
                ));
            }
        } else {
            let file = tokio::fs::File::open(&fp).await?;
            let mut reader = AsyncReaderBuilder::new().create_reader(file);
            let headers = reader
                .headers()
                .await?
                .clone();
            let mut col: HashMap<&str, usize> = HashMap::new();
            for (i, h) in headers
                .iter()
                .enumerate()
            {
                if let Some(r) = header_role(h) {
                    col.entry(r)
                        .or_insert(i);
                }
            }
            let ti = *col
                .get("title")
                .unwrap_or(&0usize);
            let ai = *col
                .get("artist")
                .unwrap_or(&0usize);
            let (mut sc, mut mc) = (0u64, 0u64);
            let mut records = reader.records();
            while let Some(rec) = records
                .next()
                .await
            {
                let rec = match rec {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                sc += 1;
                let (Some(a), Some(t)) = (rec.get(ai), rec.get(ti)) else {
                    continue;
                };
                let Some(ids) = lib.get(&match_key(a, t)) else {
                    continue;
                };
                let mut v: [Option<f64>; 10] = [None; 10];
                for (fi, fn_) in FEATURES
                    .iter()
                    .enumerate()
                {
                    if let Some(&ci) = col.get(fn_) {
                        if let Ok(raw) = rec
                            .get(ci)
                            .unwrap_or("")
                            .trim()
                            .parse::<f64>()
                        {
                            v[fi] = Some(norm_feature(fn_, raw));
                        }
                    }
                }
                if v[0..9]
                    .iter()
                    .any(|x| x.is_none())
                {
                    continue;
                }
                let p = v[9].unwrap_or(0.5);
                for id in ids {
                    let _ = sqlx::query("INSERT OR REPLACE INTO media_features(media_id,danceability,energy,valence,tempo,acousticness,instrumentalness,loudness,speechiness,liveness,popularity) VALUES(?,?,?,?,?,?,?,?,?,?,?)").bind(id).bind(v[0]).bind(v[1]).bind(v[2]).bind(v[3]).bind(v[4]).bind(v[5]).bind(v[6]).bind(v[7]).bind(v[8]).bind(p).execute(db).await;
                    mc += 1;
                }
            }
            (sc, mc)
        };
        let _ = tokio::fs::remove_file(&fp).await;
        total_sc += sc;
        total_mc += mc;
    }
    for (_, grp) in [("album", "t.parent_id"), ("artist", "t.grandparent_id")] {
        let _ = sqlx::query(&format!("INSERT OR REPLACE INTO media_features(media_id,danceability,energy,valence,tempo,acousticness,instrumentalness,loudness,speechiness,liveness,popularity) SELECT {grp},AVG(f.danceability),AVG(f.energy),AVG(f.valence),AVG(f.tempo),AVG(f.acousticness),AVG(f.instrumentalness),AVG(f.loudness),AVG(f.speechiness),AVG(f.liveness),AVG(f.popularity) FROM media_features f JOIN media t ON t.id=f.media_id WHERE t.kind='track' AND {grp} IS NOT NULL GROUP BY {grp}")).execute(db).await;
    }
    Ok(ImportStatus {
        scanning: false,
        scanned: total_sc,
        matched: total_mc,
        total_tracks: total,
    })
}

#[cfg(feature = "import-parquet")]
async fn import_parquet(
    db: &sqlx::SqlitePool,
    path: &str,
    lib: &HashMap<String, Vec<Uuid>>,
) -> std::result::Result<(u64, u64), anyhow::Error> {
    use parquet::file::reader::{FileReader, SerializedFileReader};
    let file = std::fs::File::open(path)?;
    let reader = SerializedFileReader::new(file)?;
    let schema = reader
        .metadata()
        .file_metadata()
        .schema_descr();
    let mut role_col: HashMap<&str, String> = HashMap::new();
    for i in 0..schema.num_columns() {
        let name = schema
            .column(i)
            .name()
            .to_string();
        if let Some(role) = header_role(&name) {
            role_col
                .entry(role)
                .or_insert(name);
        }
    }
    let tc = role_col
        .get("title")
        .cloned()
        .unwrap_or_default();
    let ac = role_col
        .get("artist")
        .cloned()
        .unwrap_or_default();
    let (mut sc, mut mc) = (0u64, 0u64);
    for row in reader.get_row_iter(None)? {
        let row = match row {
            Ok(r) => r,
            Err(_) => continue,
        };
        sc += 1;
        let mut map: HashMap<String, String> = HashMap::new();
        for (name, field) in row.get_column_iter() {
            map.insert(name.clone(), field.to_string());
        }
        let (Some(a), Some(t)) = (map.get(&ac), map.get(&tc)) else {
            continue;
        };
        let Some(ids) = lib.get(&match_key(a, t)) else {
            continue;
        };
        let mut v: [Option<f64>; 10] = [None; 10];
        for (fi, fn_) in FEATURES
            .iter()
            .enumerate()
        {
            if let Some(cn) = role_col.get(*fn_) {
                if let Some(raw) = map
                    .get(cn)
                    .and_then(|x| {
                        x.trim()
                            .parse::<f64>()
                            .ok()
                    })
                {
                    v[fi] = Some(norm_feature(fn_, raw));
                }
            }
        }
        if v[0..9]
            .iter()
            .any(|x| x.is_none())
        {
            continue;
        }
        let p = v[9].unwrap_or(0.5);
        for id in ids {
            let _=sqlx::query("INSERT OR REPLACE INTO media_features(media_id,danceability,energy,valence,tempo,acousticness,instrumentalness,loudness,speechiness,liveness,popularity) VALUES(?,?,?,?,?,?,?,?,?,?,?)").bind(id).bind(v[0]).bind(v[1]).bind(v[2]).bind(v[3]).bind(v[4]).bind(v[5]).bind(v[6]).bind(v[7]).bind(v[8]).bind(p).execute(db).await;
            mc += 1;
        }
    }
    Ok((sc, mc))
}

#[post("/system/importaudiofeatures")]
pub async fn start_import(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    let url = db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await?
    .feature_dataset_url
    .unwrap_or_default();
    if url.is_empty() {
        return Ok(Json(
            serde_json::json!({"ok": false, "error": "no FeatureDatasetUrl configured"}),
        ));
    }
    let st = do_import(
        &state
            .ctx
            .db,
        &url,
    )
    .await?;
    Ok(Json(
        serde_json::json!({"ok": true, "scanned": st.scanned, "matched": st.matched, "library_tracks": st.total_tracks}),
    ))
}

#[get("/system/importaudiofeatures")]
pub async fn import_status(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<Json<ImportStatus>> {
    let (mc,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM media_features f JOIN media m ON m.id = f.media_id WHERE m.kind = 'track'").fetch_one(&state.ctx.db).await?;
    let (tot,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM media WHERE kind = 'track'")
            .fetch_one(
                &state
                    .ctx
                    .db,
            )
            .await?;
    Ok(Json(ImportStatus {
        scanning: false,
        scanned: 0,
        matched: mc as u64,
        total_tracks: tot as u64,
    }))
}
