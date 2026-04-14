use anyhow::Result;
use std::path::PathBuf;
use tray_icon::{
    TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

#[cfg(dashboard_built)]
include!(concat!(env!("OUT_DIR"), "/dashboard_embed.rs"));

#[cfg(jellyfin_web_built)]
include!(concat!(env!("OUT_DIR"), "/jellyfin_web_embed.rs"));

#[cfg(anfiteatro_web_built)]
include!(concat!(env!("OUT_DIR"), "/anfiteatro_web_embed.rs"));

fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("remux")
}

fn build_config() -> remux_server::Config {
    let base = data_dir();
    let db_path = base.join("db.sqlite");
    remux_server::Config {
        database_url: format!("sqlite://{}?mode=rwc", db_path.display()),
        log_file: base
            .join("logs")
            .join("remux.jsonl")
            .to_string_lossy()
            .into_owned(),
        torrent_data_dir: base.join("torrents").to_string_lossy().into_owned(),
        ..Default::default()
    }
}

fn server_url() -> String {
    let port = std::env::var("REMUX_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(3000);
    format!("http://localhost:{port}/admin")
}

fn ensure_data_dirs(config: &remux_server::Config) -> Result<()> {
    std::fs::create_dir_all(&config.torrent_data_dir)?;
    if let Some(parent) = std::path::Path::new(&config.log_file).parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    remux_server::setup_logging();

    // Point server at bundled jellyfin-ffmpeg binaries placed next to the exe.
    set_ffmpeg_paths();

    let config = build_config();
    ensure_data_dirs(&config)?;

    // Start the remux server in a background tokio thread with embedded assets.
    let rt = tokio::runtime::Runtime::new()?;
    let server_config = config.clone();
    std::thread::spawn(move || {
        rt.block_on(async move {
            if let Err(e) = serve(server_config).await {
                tracing::error!("server error: {e:#}");
            }
        });
    });

    let open_item = MenuItem::new("Open Remux", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let open_id = open_item.id().clone();
    let quit_id = quit_item.id().clone();

    let menu = Menu::new();
    menu.append(&open_item)?;
    menu.append(&quit_item)?;

    let icon = load_icon();

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Remux")
        .with_icon(icon)
        .build()?;

    tracing::info!("remux desktop started — tray icon active");

    let menu_channel = MenuEvent::receiver();
    loop {
        if let Ok(event) = menu_channel.try_recv() {
            if event.id == open_id {
                let url = server_url();
                tracing::info!("opening {url}");
                let _ = open::that(&url);
            } else if event.id == quit_id {
                tracing::info!("quit requested");
                std::process::exit(0);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

async fn serve(config: remux_server::Config) -> anyhow::Result<()> {
    #[cfg(anfiteatro_web_built)]
    let anfiteatro = Some(&ANFITEATRO_WEB);
    #[cfg(not(anfiteatro_web_built))]
    let anfiteatro: Option<&'static include_dir::Dir<'static>> = None;

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
    let web_client = remux_server::WebClientService::from_embedded(&JELLYFIN_WEB, anfiteatro);

    #[cfg(not(all(dashboard_built, jellyfin_web_built)))]
    let web_client = {
        let paths = remux_server::FilesystemPaths::default();
        remux_server::WebClientService::from_filesystem(&paths.web_path, &paths.anfiteatro_web_path)
    };

    let (router, _) = remux_server::init_app(config, None, admin, web_client).await?;
    remux_server::bind_and_serve(router).await
}

fn load_icon() -> tray_icon::Icon {
    tray_icon::Icon::from_rgba(vec![0u8, 0, 0, 0], 1, 1).expect("valid icon")
}

/// Detect jellyfin-ffmpeg binaries bundled next to the executable and set
/// FFMPEG_PATH / FFPROBE_PATH so the server uses them instead of system ffmpeg.
fn set_ffmpeg_paths() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else { return };

    #[cfg(target_os = "windows")]
    let (ffmpeg, ffprobe) = ("ffmpeg.exe", "ffprobe.exe");
    #[cfg(not(target_os = "windows"))]
    let (ffmpeg, ffprobe) = ("ffmpeg", "ffprobe");

    let ffmpeg_path = dir.join(ffmpeg);
    let ffprobe_path = dir.join(ffprobe);

    if ffmpeg_path.exists() {
        unsafe { std::env::set_var("FFMPEG_PATH", &ffmpeg_path) };
    }
    if ffprobe_path.exists() {
        unsafe { std::env::set_var("FFPROBE_PATH", &ffprobe_path) };
    }
}
