use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use dioxus_logger::tracing;
use dioxus_logger::tracing::{debug, info, trace, Level};
use image::codecs::png::PngEncoder;
use image::io::Reader as ImageReader;
use image::ColorType;
use image::ImageEncoder;
use image::{DynamicImage, GenericImageView, RgbaImage};
use reqwest::get;
use std::io::Cursor;
use web_sys;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

use anyhow::Result;
use dioxus::prelude::*;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

pub trait ResultLogExt<T, E> {
    fn on_err<F: FnOnce(&E)>(self, f: F) -> Self;
}

impl<T, E> ResultLogExt<T, E> for Result<T, E> {
    fn on_err<F: FnOnce(&E)>(self, f: F) -> Self {
        if let Err(ref e) = self {
            f(e);
        }
        self
    }
}

#[derive(Clone)]
pub struct PaginatedState<T: 'static> {
    pub items: Signal<Vec<T>>,
    pub offset: Signal<usize>,
    pub is_loading: Signal<bool>,
    pub has_more: Signal<bool>,
    pub error: Signal<Option<Rc<anyhow::Error>>>,
    pub load_next_cb: Callback<()>,
}

impl<T: 'static> PaginatedState<T> {
    pub fn load_next(&self) {
        self.load_next_cb.call(());
    }
}

pub fn use_paginated_resource<T, F, Fut>(limit: usize, fetch_page: F) -> Resource<PaginatedState<T>>
where
    T: 'static + Clone,
    F: Fn(usize, usize) -> Fut + Clone + 'static,
    Fut: Future<Output = Result<Vec<T>>> + 'static,
{
    let items = use_signal(|| vec![]);
    let offset = use_signal(|| 0);
    let is_loading = use_signal(|| false);
    let has_more = use_signal(|| true);
    let error = use_signal(|| None::<Rc<anyhow::Error>>);

    let fetch_page_cb = fetch_page.clone();
    let items_cb = items.clone();
    let offset_cb = offset.clone();
    let mut is_loading_cb = is_loading.clone();
    let has_more_cb = has_more.clone();
    let error_cb = error.clone();

    let load_next_cb = use_callback(move |_| {
        if *is_loading_cb.read() || !*has_more_cb.read() {
            return;
        }

        is_loading_cb.set(true);
        let fetch_page = fetch_page_cb.clone();
        let mut items = items_cb.clone();
        let mut offset = offset_cb.clone();
        let mut is_loading = is_loading_cb.clone();
        let mut has_more = has_more_cb.clone();
        let mut error = error_cb.clone();

        spawn(async move {
            let offset_val = *offset.read();
            match fetch_page(limit, offset_val).await {
                Ok(new_items) => {
                    let mut current = items.read().clone();
                    current.extend(new_items.clone());
                    items.set(current);
                    offset.set(offset_val + new_items.len());
                    has_more.set(new_items.len() >= limit);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(Rc::new(e)));
                }
            }
            is_loading.set(false);
        });
    });

    //load_next_cb.call(());
    let cb = load_next_cb.clone();

    use_future(move || async move {
        cb.call(());
    });

    use_resource(move || async move {
        PaginatedState {
            items,
            offset,
            is_loading,
            has_more,
            error,
            load_next_cb,
        }
    })
}

// Scroll logic utility
pub fn scroll_to_index(id: String) {
    //info!("SCROLLLINNGG to index: {}", id);
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        if let Some(elem) = document.get_element_by_id(&format!("{}", id)) {
            //debug!("found target");
            let mut options = web_sys::ScrollIntoViewOptions::new();
            options.behavior(web_sys::ScrollBehavior::Smooth);
            options.block(web_sys::ScrollLogicalPosition::Nearest);
            options.inline(web_sys::ScrollLogicalPosition::Start);
            elem.scroll_into_view_with_scroll_into_view_options(&options);
        }
    }
}

fn is_white_or_transparent(pixel: image::Rgba<u8>) -> bool {
    let [r, g, b, a] = pixel.0;
    a < 10 || (r > 220 && g > 220 && b > 220) // more lenient
}

fn trim_image(img: &DynamicImage) -> RgbaImage {
    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();

    let mut top = height;
    let mut left = width;
    let mut right = 0;
    let mut bottom = 0;

    for y in 0..height {
        for x in 0..width {
            let pixel = rgba.get_pixel(x, y);
            if !is_white_or_transparent(*pixel) {
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
            }
        }
    }

    if right < left || bottom < top {
        return img.to_rgba8(); // fallback to full image instead of blank
    }

    if right < left || bottom < top {
        return RgbaImage::new(1, 1); // transparent 1×1 fallback
    }

    rgba.view(left, top, right - left + 1, bottom - top + 1)
        .to_image()
}

pub async fn trim_image_from_url(url: &str) -> anyhow::Result<RgbaImage> {
    let response = get(url).await?;
    let bytes = response.bytes().await?;
    let cursor = Cursor::new(bytes);

    let img = ImageReader::new(cursor).with_guessed_format()?.decode()?;

    Ok(trim_image(&img))
}

pub fn image_to_base64_png(img: &RgbaImage) -> String {
    let mut buf = Cursor::new(Vec::new());

    PngEncoder::new(&mut buf)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ColorType::Rgba8.into(),
        )
        .expect("PNG encoding failed");

    let encoded = buf.into_inner();
    let base64 = STANDARD.encode(encoded);
    format!("data:image/png;base64,{}", base64)
}

#[tracing::instrument(level = "debug")]
pub async fn fetch_and_trim_base64(url: &str) -> Option<String> {
    let response = reqwest::get(url).await.ok()?;
    let bytes = response.bytes().await.ok()?;
    let cursor = std::io::Cursor::new(bytes);

    let img = image::io::Reader::new(cursor)
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;

    let trimmed = trim_image(&img);
    Some(image_to_base64_png(&trimmed))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = playHls)]
    pub fn play_hls(id: &str, url: &str);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn play_hls(_id: &str, _url: &str) {
    // no-op for native builds
}

pub trait TryIntoVec<U> {
    type Error;

    fn try_into_vec(self) -> Result<Vec<U>, Self::Error>;
}

impl<T, U> TryIntoVec<U> for Vec<T>
where
    T: TryInto<U>,
{
    type Error = T::Error;

    fn try_into_vec(self) -> Result<Vec<U>, Self::Error> {
        self.into_iter().map(TryInto::try_into).collect()
    }
}
