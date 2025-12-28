use crate::sdks::jellyfin;
use anyhow::{Context, Result, anyhow};
use async_compression::tokio::bufread::GzipDecoder;

//use futures::Stream;
//use futures::StreamExt;
//use futures_util::TryStreamExt;
use tokio_stream::Stream;
use tokio_stream::StreamExt;
// use tokio_stream::TryStreamExt;
//use tokio_stream::TryStreamExt;
use chrono::{DateTime, NaiveDate, Utc};
use csv_async::AsyncDeserializer;
use csv_async::AsyncReaderBuilder;
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::path::Path;
use std::pin::Pin;
//use std::task::{Context, Poll};
use tempfile;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::{
    fs::File as TokioFile,
    io::{AsyncSeekExt, AsyncWriteExt},
};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::io::{ReaderStream, StreamReader};
use tracing;
//use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use crate::errors::LogErr;
use std::str::FromStr;

use moka::sync::Cache;
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use uuid::Uuid;

pub fn server_id() -> String {
    "remux".to_string()
}

const NS_MEDIA: Uuid = uuid::uuid!("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
static MEDIA_LOOKUP: OnceLock<Cache<Uuid, MediaId>> = OnceLock::new();

fn media_lookup() -> &'static Cache<Uuid, MediaId> {
    MEDIA_LOOKUP.get_or_init(|| {
        Cache::builder()
            .max_capacity(100_000)
            .time_to_idle(std::time::Duration::from_secs(60 * 60))
            .build()
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaId {
    pub id: String,
    pub media_type: jellyfin::MediaType,
    pub uuid: Uuid,
    pub stream_id: Option<String>,
}

impl MediaId {
    pub fn new(
        id: String,
        media_type: jellyfin::MediaType,
        stream_id: Option<String>,
    ) -> Self {
        let uuid = Self::stable_uuid_for(&id, media_type, stream_id.as_deref());

        let media = Self {
            id,
            media_type,
            uuid,
            stream_id,
        };

        media_lookup().insert(uuid, media.clone());
        media
    }

    pub fn from_uuid(uuid: &Uuid) -> Result<Self> {
        media_lookup().get(uuid).map(|v| v.clone()).ok_or_else(|| {
            anyhow!("unknown jellyfin id: {uuid} (not warmed or evicted)")
        })
    }

    pub fn stable_uuid_for(
        id: &str,
        media_type: jellyfin::MediaType,
        stream_id: Option<&str>,
    ) -> Uuid {
        let name = format!("{}|{}|{}", id, media_type, stream_id.unwrap_or(""));
        Uuid::new_v5(&NS_MEDIA, name.as_bytes())
    }
}

impl Serialize for MediaId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.uuid.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MediaId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let uuid = Uuid::deserialize(deserializer)?;
        MediaId::from_uuid(&uuid).map_err(de::Error::custom)
    }
}

impl TryFrom<Uuid> for MediaId {
    type Error = anyhow::Error;

    fn try_from(uuid: Uuid) -> Result<Self, Self::Error> {
        MediaId::from_uuid(&uuid)
    }
}

//impl FromStr for MediaId {
//    type Err = anyhow::Error;

//    fn from_str(uuid: &str) -> Result<Self> {
//        MediaId::from_uuid(&uuid)
//    }
//}

//impl From<MediaId> for String {
//    fn from(media: MediaId) -> Self {
//        media.uuid.to_string()
//    }
//}

//impl TryFrom<String> for MediaId {
//    type Error = anyhow::Error;

//    fn try_from(uuid: String) -> Result<Self, Self::Error> {
//        MediaId::from_str(&token)
//    }
//}

pub fn native_to_utc(opt_date: Option<NaiveDate>) -> Option<DateTime<Utc>> {
    opt_date
        .and_then(|d| d.and_hms_opt(0, 0, 0)) // Add time
        .map(|ndt| DateTime::<Utc>::from_utc(ndt, Utc)) // Make it UTC
}
//pub static LIBRARIES: &[&str] = &["movies", "shows"];

pub fn libraries() -> Vec<jellyfin::BaseItemDto> {
    vec![
        jellyfin::BaseItemDto {
            name: Some("Movies".to_string()),
            id: MediaId::new(
                "movies".into(),
                jellyfin::MediaType::CollectionFolder,
                None,
            ),
            type_: Some(jellyfin::MediaType::CollectionFolder),
            collection_type: Some(jellyfin::CollectionType::Movies),
            is_folder: Some(true),
            //image_tags: Some(jellyfin::ImageTags {
            //    primary: Some("jsjsj".to_string()),
            //    ..Default::default()
            //}),
            ..Default::default()
        },
        jellyfin::BaseItemDto {
            name: Some("Series".to_string()),
            //id: "series".to_string(),
            id: MediaId::new(
                "series".into(),
                jellyfin::MediaType::CollectionFolder,
                None,
            ),
            //parent_id: Some("test".to_string()),
            type_: Some(jellyfin::MediaType::CollectionFolder),
            collection_type: Some(jellyfin::CollectionType::Tvshows),
            is_folder: Some(true),
            ..Default::default()
        },
        jellyfin::BaseItemDto {
            name: Some("Collections".to_string()),
            //id: "collections".to_string(),
            id: MediaId::new(
                "collections".into(),
                jellyfin::MediaType::CollectionFolder,
                None,
            ),
            //parent_id: Some("test".to_string()),
            type_: Some(jellyfin::MediaType::CollectionFolder),
            collection_type: Some(jellyfin::CollectionType::Boxsets),
            is_folder: Some(true),
            ..Default::default()
        },
    ]
}

pub async fn download_to_file(url: &str) -> Result<TokioFile> {
    let resp = reqwest::get(url).await?.error_for_status()?;
    let bytes = resp.bytes().await?;

    let std_file = tempfile::tempfile()?; // std::fs::File
    let mut file = TokioFile::from_std(std_file); // convert to async
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    file.seek(std::io::SeekFrom::Start(0)).await?;

    Ok(file)
}

pub struct FileStream<T> {
    inner: Pin<Box<dyn Stream<Item = Result<T>> + Send>>,
}

impl<T> FileStream<T>
where
    T: DeserializeOwned + Send + 'static,
{
    pub async fn from_url(url: &str) -> Result<Self> {
        let tmpfile = download_to_file(url).await?;

        // detect extension (gzip-inside)
        let path = Path::new(url);
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let inner_ext = ext.trim_end_matches(".gz");

        let reader = BufReader::new(tmpfile);
        let decoder = GzipDecoder::new(reader);
        //let buffered: Box<dyn tokio::io::AsyncBufRead + Send + Unpin> =
        //    Box::new(BufReader::new(decoder));
        let buffered = BufReader::new(decoder);

        // JSON-lines
        if matches!(inner_ext, "json" | "jsonl" | "ndjson") {
            let line_stream =
                tokio_stream::wrappers::LinesStream::new(buffered.lines());
            let json_stream = line_stream
                .then(|line_result| async move {
                    match line_result {
                        Ok(line) => match serde_json::from_str::<T>(&line) {
                            Ok(obj) => Some(Ok(obj)),
                            Err(e) => {
                                tracing::warn!("Line read error: {e} — skipping line");
                                None
                            }
                        },
                        Err(e) => {
                            tracing::warn!("Line read error: {e} — skipping line");
                            None
                        }
                    }
                })
                .filter_map(|x| x);

            return Ok(Self {
                inner: Box::pin(json_stream),
            });
        }

        // CSV/TSV fallback
        // let delimiter = if inner_ext == "tsv" { b'\t' } else { b',' };
        let delimiter = b'\t';
        let csv_reader = AsyncReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(true)
            .create_deserializer(buffered);
        // .create_reader(buffered);

        let csv_stream = csv_reader
            .into_deserialize::<T>() // <-- note: deserialize, not deserializer
            .then(|res| async move {
                match res {
                    Ok(row) => {
                        // tracing::info!("sucess");
                        Some(Ok(row))
                    }
                    Err(e) => {
                        tracing::warn!("CSV parse error: {e} — skipping row");
                        None
                    }
                }
            })
            .filter_map(|x| x);
        // let csv_stream = AsyncReaderBuilder::new()
        //     .delimiter(delimiter)
        //     .has_headers(true)
        //     .create_deserializer(reader)
        //     .deserialize::<T>()
        //     .then(|res| async move {
        //         match res {
        //             Ok(row) => Some(Ok(row)),
        //             Err(e) => {
        //                 tracing::warn!("CSV parse error: {e} — skipping row");
        //                 None
        //             }
        //         }
        //     })
        //     .filter_map(|x| x);

        Ok(Self {
            inner: Box::pin(csv_stream),
        })
    }
}

pub fn parse_strings_to_u64s(strings: Vec<String>) -> Vec<u64> {
    strings
        .into_iter()
        .filter_map(|s| s.parse::<u64>().ok())
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub enum TickUnit {
    Ticks,
    Seconds,
    Minutes,
}

impl std::str::FromStr for TickUnit {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ticks" => Ok(TickUnit::Ticks),
            "seconds" => Ok(TickUnit::Seconds),
            "minutes" => Ok(TickUnit::Minutes),
            _ => Err(()),
        }
    }
}

pub fn duration_to_ticks(value: f64, unit: TickUnit) -> i64 {
    match unit {
        TickUnit::Ticks => value.round() as i64,
        TickUnit::Seconds => (value * 10_000_000.0).round() as i64,
        TickUnit::Minutes => (value * 60.0 * 10_000_000.0).round() as i64,
    }
}

pub trait ToRunTimeTicks {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64>;
}

// Numeric types
impl ToRunTimeTicks for u32 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self as f64, unit))
    }
}

impl ToRunTimeTicks for u64 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self as f64, unit))
    }
}

impl ToRunTimeTicks for i32 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self as f64, unit))
    }
}

impl ToRunTimeTicks for i64 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self as f64, unit))
    }
}

impl ToRunTimeTicks for f64 {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        Some(duration_to_ticks(*self, unit))
    }
}

// Strings
impl ToRunTimeTicks for String {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        self.parse::<f64>().ok().and_then(|v| v.to_ticks(unit))
    }
}

impl ToRunTimeTicks for &str {
    fn to_ticks(&self, unit: TickUnit) -> Option<i64> {
        self.parse::<f64>().ok().and_then(|v| v.to_ticks(unit))
    }
}

pub fn get_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
