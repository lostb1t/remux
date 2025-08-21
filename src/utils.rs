use crate::db;
use crate::sdks::jellyfin;
use async_compression::tokio::bufread::GzipDecoder;
use eyre;
use eyre::Result;
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
use std::task::{Context, Poll};
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
use std::str::FromStr;
use crate::errors::LogErr;

//use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

use base36::{decode, encode};

pub fn encode_media_uuid(id: &str, media_type: jellyfin::MediaType) -> String {
    encode(format!("{id}:{media_type}").as_bytes()) // produces lowercase [0-9a-z]
}

pub fn decode_media_uuid(encoded: &str) -> Result<(String, jellyfin::MediaType)> {
    let bytes = decode(encoded).anyhow()?; // Vec<u8>
    let s = std::str::from_utf8(&bytes)?; // panic-safe UTF-8 check

    let (id, media_type) = s.rsplit_once(':').unwrap();

    Ok((id.to_string(), jellyfin::MediaType::from_str(media_type)?))
}

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
            id: "movies".to_string(),
            //parent_id: Some("test".to_string()),
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
            id: "series".to_string(),
            //parent_id: Some("test".to_string()),
            type_: Some(jellyfin::MediaType::CollectionFolder),
            collection_type: Some(jellyfin::CollectionType::Tvshows),
            is_folder: Some(true),
            ..Default::default()
        },
        jellyfin::BaseItemDto {
            name: Some("Collections".to_string()),
            id: "collections".to_string(),
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

impl<T> Stream for FileStream<T>
where
    T: DeserializeOwned + Send + 'static,
{
    type Item = Result<T>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
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
