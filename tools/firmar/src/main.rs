//! Firma de los instaladores de Michi (las actualizaciones, ver
//! src/update.rs del navegador).
//!
//! Michi solo abre un instalador descargado si viene con su `.sig` y la firma
//! corresponde a la clave publica que lleva adentro (UPDATE_KEY). La huella
//! que publica GitHub sola no alcanza: viene del mismo lugar que el archivo,
//! asi que quien se quede con la cuenta o el repositorio podia publicar un
//! instalador cualquiera con su huella. La clave privada nunca esta en GitHub:
//! vive en la PC de quien publica las versiones.
//!
//! Uso:
//!   michi-firmar clave <clave.pk8>             crea el par de claves (una sola vez)
//!   michi-firmar firmar <instalador.exe> <clave.pk8>   deja <instalador.exe>.sig
//!
//! Lo firmado es "Michi-actualizacion-v1\0" + el SHA-256 del instalador
//! (Ed25519), y el .sig es esa firma en base64.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine;
use ring::digest::{Context, SHA256};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};

/// Lo mismo que SIGNATURE_CONTEXT en src/update.rs.
const CONTEXT: &[u8] = b"Michi-actualizacion-v1\0";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["clave", key] => new_key(Path::new(key)),
        ["firmar", file, key] => sign(Path::new(file), Path::new(key)),
        _ => Err("uso: michi-firmar clave <clave.pk8> | firmar <instalador.exe> <clave.pk8>".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn new_key(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Err(format!("{} ya existe: no se pisa una clave", path.display()));
    }
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| "no se pudo generar la clave")?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "clave invalida")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, pkcs8.as_ref()).map_err(|e| e.to_string())?;
    let public: Vec<String> = pair.public_key().as_ref().iter().map(|b| format!("0x{b:02x}")).collect();
    println!("Clave privada: {} (guardala en un lugar seguro y con copia)", path.display());
    println!("Clave publica, para UPDATE_KEY en src/update.rs:");
    println!("[{}]", public.join(", "));
    Ok(())
}

fn sign(file: &Path, key: &Path) -> Result<(), String> {
    let pkcs8 = std::fs::read(key).map_err(|e| format!("no se pudo leer la clave {}: {e}", key.display()))?;
    let pair = Ed25519KeyPair::from_pkcs8(&pkcs8).map_err(|_| "la clave no es valida")?;
    let digest = sha256(file)?;
    let message = [CONTEXT, digest.as_slice()].concat();
    let signature = base64::engine::general_purpose::STANDARD.encode(pair.sign(&message).as_ref());
    let mut out = PathBuf::from(file).into_os_string();
    out.push(".sig");
    std::fs::write(&out, format!("{signature}\n")).map_err(|e| e.to_string())?;
    println!("Firmado: {}", Path::new(&out).display());
    Ok(())
}

fn sha256(file: &Path) -> Result<Vec<u8>, String> {
    let mut f = std::fs::File::open(file).map_err(|e| format!("no se pudo abrir {}: {e}", file.display()))?;
    let mut ctx = Context::new(&SHA256);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(ctx.finish().as_ref().to_vec())
}
