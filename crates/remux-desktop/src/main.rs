use anyhow::Result;
use std::path::PathBuf;
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
    let quit_item = MenuItem::new("Quit", true, None);
    let open_id = open_item
        .id()
        .clone();
    let quit_id = quit_item
        .id()
        .clone();

    let menu = Menu::new();
    menu.append(&open_item)?;
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
    let img = image::load_from_memory(bytes)
        .expect("valid icon")
        .resize(32, 32, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).expect("valid icon")
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
