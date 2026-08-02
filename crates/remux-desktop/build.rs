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
    println!("cargo:rerun-if-changed={}", dashboard_dir.display());
    if dashboard_dir.exists() {
        let path = dashboard_dir
            .canonicalize()
            .unwrap();
        let path_str = canonicalize_for_include(&path);
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

    // Accept either jellyfin-web/dist (local npm build) or jellyfin-web/ (CI artifact layout).
    let jellyfin_web_dir = ["jellyfin-web/dist", "jellyfin-web"]
        .iter()
        .map(|p| workspace_root.join(p))
        .find(|p| {
            p.join("index.html")
                .exists()
        })
        .unwrap_or_else(|| workspace_root.join("jellyfin-web/dist"));
    println!("cargo:rerun-if-changed={}", jellyfin_web_dir.display());
    if jellyfin_web_dir.exists() {
        let path = jellyfin_web_dir
            .canonicalize()
            .unwrap();
        let path_str = canonicalize_for_include(&path);
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

/// Convert a canonicalized path to a forward-slash string suitable for `include_dir!`.
/// On Windows, `canonicalize` returns a UNC path like `\\?\C:\...` which becomes
/// `//?/C:/...` after naive backslash replacement — not a valid path for the macro.
/// Strip the UNC prefix so we get a plain `C:/...` path instead.
fn canonicalize_for_include(path: &std::path::Path) -> String {
    let s = path
        .to_str()
        .unwrap()
        .replace('\\', "/");
    // Strip Windows extended-length path prefix \\?\ (→ //?/ after replacement).
    s.trim_start_matches("//?/")
        .to_string()
}
