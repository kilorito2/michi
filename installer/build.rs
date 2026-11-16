//! El mismo icono del navegador para el instalador (installer/setup.rc) y la
//! decision de incrustar o no los fondos animados.
//!
//! Los fondos (assets/wallpapers/optimized) son de terceros y no estan en el
//! repositorio: sin ellos el instalador igual tiene que compilar (un clon
//! limpio, GitHub Actions). Si estan todos, se emite `cfg(fondos)` y
//! main.rs los incrusta; si falta alguno, la pagina usa un degradado CSS de
//! reserva. tools/build_installer.ps1 define MICHI_FONDOS_OBLIGATORIOS para
//! que el instalador que se publica nunca salga sin fondos por descuido.

use std::path::Path;

/// Los mismos archivos que incrusta main.rs (THEMES).
const FONDOS: &[&str] = &[
    "sleepy-rainy-evening.blur.mp4",
    "torii-carmesi.blur.jpg",
    "blindfolded-girl.blur.mp4",
    "wuthering-waves-chisa.blur.mp4",
];

fn main() {
    println!("cargo:rerun-if-changed=setup.rc");
    println!("cargo:rerun-if-changed=../assets/icon/michi.ico");
    println!("cargo:rerun-if-env-changed=MICHI_FONDOS_OBLIGATORIOS");
    println!("cargo::rustc-check-cfg=cfg(fondos)");

    let dir = Path::new("../assets/wallpapers/optimized");
    let missing: Vec<&str> = FONDOS.iter().copied().filter(|f| !dir.join(f).is_file()).collect();
    for f in FONDOS {
        // Si el archivo no existe, cargo vuelve a correr este script en cada
        // compilacion: asi se entera cuando aparecen los fondos.
        println!("cargo:rerun-if-changed=../assets/wallpapers/optimized/{f}");
    }
    if missing.is_empty() {
        println!("cargo:rustc-cfg=fondos");
    } else if std::env::var_os("MICHI_FONDOS_OBLIGATORIOS").is_some() {
        panic!(
            "faltan fondos en assets/wallpapers/optimized: {}. Generalos con tools/encode_wallpapers.sh",
            missing.join(", ")
        );
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("setup.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("no se pudo incrustar el icono");
    }
}
