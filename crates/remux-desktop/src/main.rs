mod ffmpeg;

use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tao::event_loop::{ControlFlow, EventLoop};
use tray_icon::{
    TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

#[cfg(dashboard_built)]
include!(concat!(env!("OUT_DIR"), "/dashboard_embed.rs"));

#[cfg(all(dashboard_built, jellyfin_web_built))]
include!(concat!(env!("OUT_DIR"), "/jellyfin_web_embed.rs"));

fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("remux")
}

fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

fn build_config() -> remux_server::Config {
    let base = data_dir();
    remux_server::Config {
        data_dir: base,
        ..Default::default()
    }
    .resolve()
}

fn server_url() -> String {
    let port = build_config().port;
    format!("http://localhost:{port}/admin")
}

fn ensure_data_dirs(config: &remux_server::Config) -> Result<()> {
    std::fs::create_dir_all(
        config
            .torrent_data_dir
            .as_deref()
            .unwrap_or_default(),
    )?;
    Ok(())
}

fn cleanup_old_logs(dir: &Path) {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(5 * 24 * 3600))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name.starts_with("remux-") && name.ends_with(".log") {
            if let Ok(meta) = entry.metadata() {
                if meta
                    .modified()
                    .map(|t| t < cutoff)
                    .unwrap_or(false)
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

fn main() -> Result<()> {
    let log_dir = log_dir();
    std::fs::create_dir_all(&log_dir)?;
    cleanup_old_logs(&log_dir);
    remux_server::setup_logging(Some(&log_dir));

    let config = build_config();
    ensure_data_dirs(&config)?;

    // Start the remux server in a background tokio thread with embedded assets.
    let rt = tokio::runtime::Runtime::new()?;
    let server_config = config.clone();
    let data_dir_for_ffmpeg = config
        .data_dir
        .clone();
    std::thread::spawn(move || {
        rt.block_on(async move {
            if let Err(e) = ffmpeg::ensure_ffmpeg(&data_dir_for_ffmpeg).await {
                tracing::warn!("ffmpeg setup failed: {e:#}");
            }
            if let Err(e) = serve(server_config).await {
                tracing::error!("server error: {e:#}");
            }
        });
    });

    // Build event loop. On macOS set Accessory policy so the app has no Dock icon
    // and doesn't appear in the Cmd+Tab switcher.
    let event_loop = {
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
            let mut el = EventLoop::new();
            el.set_activation_policy(ActivationPolicy::Accessory);
            el
        }
        #[cfg(not(target_os = "macos"))]
        EventLoop::new()
    };

    let open_item = MenuItem::new("Open", true, None);
    let logs_item = MenuItem::new("Logs", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let open_id = open_item
        .id()
        .clone();
    let logs_id = logs_item
        .id()
        .clone();
    let quit_id = quit_item
        .id()
        .clone();

    let menu = Menu::new();
    menu.append(&open_item)?;
    menu.append(&logs_item)?;
    menu.append(&quit_item)?;

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Remux")
        .with_icon(load_icon())
        .build()?;

    tracing::info!("remux desktop started — tray icon active");

    let menu_channel = MenuEvent::receiver();
    let url = server_url();

    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Ok(ev) = menu_channel.try_recv() {
            if ev.id == open_id {
                tracing::info!("opening {url}");
                let _ = open::that(&url);
            } else if ev.id == logs_id {
                let _ = open::that(&log_dir);
            } else if ev.id == quit_id {
                tracing::info!("quit");
                *control_flow = ControlFlow::Exit;
            }
        }
    });
}

async fn serve(config: remux_server::Config) -> anyhow::Result<()> {
    #[cfg(all(dashboard_built, jellyfin_web_built))]
    let admin = remux_server::embedded_static::EmbeddedDir {
        dir: &DASHBOARD,
        spa_fallback: true,
    }
    .into_admin_service();

    #[cfg(not(all(dashboard_built, jellyfin_web_built)))]
    let admin = remux_server::admin_from_filesystem(
        &remux_server::FilesystemPaths::default().dashboard_path,
    );

    #[cfg(all(dashboard_built, jellyfin_web_built))]
    let web_client = remux_server::WebClientService::from_embedded(&JELLYFIN_WEB);

    #[cfg(not(all(dashboard_built, jellyfin_web_built)))]
    let web_client = {
        let paths = remux_server::FilesystemPaths::default();
        remux_server::WebClientService::from_filesystem(&paths.web_path)
    };

    let port = config.port;
    let (router, _) = remux_server::init_app(config, None, admin, web_client).await?;
    remux_server::bind_and_serve(router, port).await
}

fn load_icon() -> tray_icon::Icon {
    let bytes = include_bytes!("../../../logo.png");
    let img = image::load_from_memory(bytes).expect("valid icon");

    // Crop bottom 25% to remove the "REMUX" wordmark, keeping only the R mark.
    let (fw, fh) = (img.width(), img.height());
    let icon_only = img.crop_imm(0, 0, fw, fh * 3 / 4);

    #[cfg(target_os = "macos")]
    {
        // macOS menu bar requires black-on-transparent template image.
        // The OS then renders it white (dark mode) or black (light mode) automatically.
        let resized = icon_only
            .resize(22, 22, image::imageops::FilterType::Lanczos3)
            .into_rgba8();
        let (w, h) = resized.dimensions();
        let mut out = image::RgbaImage::new(w, h);
        for (x, y, pixel) in resized.enumerate_pixels() {
            let [r, g, b, _] = pixel.0;
            let luma = (r as u16 * 3 + g as u16 * 3 + b as u16) / 7;
            let alpha = ((luma.saturating_sub(60)) * 4).min(255) as u8;
            out.put_pixel(x, y, image::Rgba([0, 0, 0, alpha]));
        }
        return tray_icon::Icon::from_rgba(out.into_raw(), w, h).expect("valid icon");
    }

    // Windows / Linux: keep the original golden color, just strip the dark green background.
    #[allow(unreachable_code)]
    {
        let resized = icon_only
            .resize(32, 32, image::imageops::FilterType::Lanczos3)
            .into_rgba8();
        let (w, h) = resized.dimensions();
        let mut out = image::RgbaImage::new(w, h);
        for (x, y, pixel) in resized.enumerate_pixels() {
            let [r, g, b, _] = pixel.0;
            let luma = (r as u16 + g as u16 + b as u16) / 3;
            // Ramp alpha: dark (background) → transparent, bright (golden R) → opaque.
            let alpha = ((luma.saturating_sub(60)) * 4).min(255) as u8;
            out.put_pixel(x, y, image::Rgba([r, g, b, alpha]));
        }
        tray_icon::Icon::from_rgba(out.into_raw(), w, h).expect("valid icon")
    }
}
