//! Incrusta el icono del navegador en el .exe (assets/icon/michi.rc).
//! Para regenerar el .ico desde el PNG original: tools/make_icon.ps1.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/michi.rc");
    println!("cargo:rerun-if-changed=assets/icon/michi.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/icon/michi.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("no se pudo incrustar el icono");
    }
}
