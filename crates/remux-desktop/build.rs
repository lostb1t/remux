use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(dashboard_built)");
    println!("cargo:rustc-check-cfg=cfg(jellyfin_web_built)");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-env-changed=DASHBOARD_PATH");
    if let Ok(p) = env::var("DASHBOARD_PATH") {
        let dir = PathBuf::from(&p);
        println!("cargo:rerun-if-changed={}", dir.display());
        if dir
            .join("index.html")
            .exists()
        {
            let path_str = to_include_path(&dir);
            std::fs::write(
                out_dir.join("dashboard_embed.rs"),
                format!(r#"static DASHBOARD: include_dir::Dir<'static> = include_dir::include_dir!("{path_str}");"#),
            ).unwrap();
            println!("cargo:rustc-cfg=dashboard_built");
        } else {
            println!("cargo:error=DASHBOARD_PATH={p} does not contain index.html");
        }
    } else {
        println!(
            "cargo:warning=DASHBOARD_PATH not set — dashboard will not be embedded"
        );
    }

    println!("cargo:rerun-if-env-changed=WEB_PATH");
    if let Ok(p) = env::var("WEB_PATH") {
        let dir = PathBuf::from(&p);
        println!("cargo:rerun-if-changed={}", dir.display());
        if dir
            .join("index.html")
            .exists()
        {
            let path_str = to_include_path(&dir);
            std::fs::write(
                out_dir.join("jellyfin_web_embed.rs"),
                format!(r#"static JELLYFIN_WEB: include_dir::Dir<'static> = include_dir::include_dir!("{path_str}");"#),
            ).unwrap();
            println!("cargo:rustc-cfg=jellyfin_web_built");
        } else {
            println!("cargo:error=WEB_PATH={p} does not contain index.html");
        }
    } else {
        println!("cargo:warning=WEB_PATH not set — jellyfin-web will not be embedded");
    }
}

fn to_include_path(path: &std::path::Path) -> String {
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());
    let s = canonical
        .to_str()
        .unwrap()
        .replace('\\', "/");
    s.trim_start_matches("//?/")
        .to_string()
}
