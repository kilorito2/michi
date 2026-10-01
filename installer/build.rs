//! El mismo icono del navegador para el instalador (installer/setup.rc).

fn main() {
    println!("cargo:rerun-if-changed=setup.rc");
    println!("cargo:rerun-if-changed=../assets/icon/michi.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("setup.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("no se pudo incrustar el icono");
    }
}
