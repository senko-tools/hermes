//! Puts the app icon in the Windows executable: gpui takes the window's icon from there.

fn main() {
    println!("cargo:rerun-if-changed=hermes.rc");
    println!("cargo:rerun-if-changed=assets/icons/hermes.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("hermes.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("the app icon compiles");
    }
}
