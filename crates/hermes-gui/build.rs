//! Puts the app icon in the Windows executable (gpui takes the window's icon from there) and lists
//! the bundled Material icons for `src/assets.rs`.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=hermes.rc");
    println!("cargo:rerun-if-changed=assets/icons/hermes.ico");
    println!("cargo:rerun-if-changed=assets/icons/mdi");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("hermes.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("the app icon compiles");
    }

    let root = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets this");
    let dir = Path::new(&root).join("assets/icons/mdi");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("the icons exist")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "svg" {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_owned)
        })
        .collect();
    names.sort();
    let mut code = String::from("const ICONS: &[(&str, &[u8])] = &[\n");
    for name in names {
        let file = dir.join(format!("{name}.svg"));
        let _ = writeln!(
            code,
            "    (\"hermes/{name}.svg\", include_bytes!({:?})),",
            file.display().to_string()
        );
    }
    code.push_str("];\n");
    let out = std::env::var("OUT_DIR").expect("cargo sets this");
    std::fs::write(Path::new(&out).join("icons.rs"), code).expect("OUT_DIR is writable");
}
