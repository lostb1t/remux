use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(dashboard_built)");
    println!("cargo:rustc-check-cfg=cfg(jellyfin_web_built)");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .join("..")
        .join("..");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let dashboard_dir =
        workspace_root.join("target/dx/remux-dashboard/release/web/public");
    if dashboard_dir.exists() {
        let path = dashboard_dir
            .canonicalize()
            .unwrap();
        let path_str = path
            .to_str()
            .unwrap()
            .replace('\\', "/");
        std::fs::write(
            out_dir.join("dashboard_embed.rs"),
            format!(r#"static DASHBOARD: include_dir::Dir<'static> = include_dir::include_dir!("{path_str}");"#),
        ).unwrap();
        println!("cargo:rustc-cfg=dashboard_built");
        println!("cargo:rerun-if-changed={path_str}");
    } else {
        println!(
            "cargo:warning=Dashboard not built — run `dx build --release` in crates/remux-dashboard first"
        );
    }

    let jellyfin_web_dir = workspace_root.join("jellyfin-web/dist");
    if jellyfin_web_dir.exists() {
        let path = jellyfin_web_dir
            .canonicalize()
            .unwrap();
        let path_str = path
            .to_str()
            .unwrap()
            .replace('\\', "/");
        std::fs::write(
            out_dir.join("jellyfin_web_embed.rs"),
            format!(r#"static JELLYFIN_WEB: include_dir::Dir<'static> = include_dir::include_dir!("{path_str}");"#),
        ).unwrap();
        println!("cargo:rustc-cfg=jellyfin_web_built");
        println!("cargo:rerun-if-changed={path_str}");
    } else {
        println!(
            "cargo:warning=jellyfin-web not built — run `cargo make jellyfin-web` first"
        );
    }
}
