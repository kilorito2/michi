//! Los archivos del navegador viajan pegados al final del propio .exe del
//! instalador (los agrega tools/build_installer.ps1):
//!
//!   [instalador][archivo 1][archivo 2]...[indice JSON][largo del indice: u64 LE]["MICHISET"]
//!
//! Asi el desinstalador es este mismo programa sin esa cola (los primeros
//! `stub_len` bytes): no ocupa otros 40 MB. Cada archivo lleva su SHA-256 y se
//! verifica al extraerlo: un instalador cortado o danado no deja una
//! instalacion a medias.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use ring::digest::{Context, SHA256};
use serde::Deserialize;

const MAGIC: &[u8; 8] = b"MICHISET";
const CHUNK: usize = 1 << 20;

#[derive(Deserialize)]
pub struct Index {
    pub version: String,
    /// Bytes del instalador en si (lo que se copia como desinstalador).
    pub stub_len: u64,
    pub files: Vec<Entry>,
}

#[derive(Deserialize)]
pub struct Entry {
    /// Ruta relativa a la carpeta de instalacion ("michi.exe", "wallpapers/x.mp4").
    pub path: String,
    pub offset: u64,
    pub len: u64,
    pub sha256: String,
}

pub struct Payload {
    exe: PathBuf,
    pub index: Index,
}

impl Payload {
    /// Los archivos pegados a este .exe, si los tiene (el desinstalador no).
    pub fn open() -> Option<Payload> {
        let exe = std::env::current_exe().ok()?;
        let mut file = File::open(&exe).ok()?;
        let size = file.metadata().ok()?.len();
        let mut footer = [0u8; 16];
        file.seek(SeekFrom::Start(size.checked_sub(16)?)).ok()?;
        file.read_exact(&mut footer).ok()?;
        if &footer[8..] != MAGIC {
            return None;
        }
        let index_len = u64::from_le_bytes(footer[..8].try_into().ok()?);
        let index_start = size.checked_sub(16 + index_len)?;
        let mut json = vec![0; usize::try_from(index_len).ok()?];
        file.seek(SeekFrom::Start(index_start)).ok()?;
        file.read_exact(&mut json).ok()?;
        let index: Index = serde_json::from_slice(&json).ok()?;
        let fits = index.files.iter().all(|f| f.offset.checked_add(f.len).is_some_and(|end| end <= index_start));
        (fits && index.files.iter().all(|f| safe_relative(&f.path).is_some())).then_some(Payload { exe, index })
    }

    pub fn total_bytes(&self) -> u64 {
        self.index.files.iter().map(|f| f.len).sum()
    }

    /// Extrae todo en `dest`. Cada archivo se escribe primero como
    /// "<nombre>.nuevo" y recien reemplaza al anterior si su SHA-256 coincide.
    /// `progress(bytes hechos, archivo actual)`.
    pub fn extract(&self, dest: &Path, mut progress: impl FnMut(u64, &str)) -> Result<(), String> {
        let damaged = "El instalador esta danado o incompleto: vuelve a descargarlo.";
        let mut src = File::open(&self.exe).map_err(|e| e.to_string())?;
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK];
        for entry in &self.index.files {
            let rel = safe_relative(&entry.path).ok_or(damaged)?;
            let target = dest.join(&rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("No se pudo crear {} ({e}).", parent.display()))?;
            }
            let temp = PathBuf::from(format!("{}.nuevo", target.display()));
            let mut out = File::create(&temp).map_err(|e| format!("No se pudo escribir {} ({e}).", temp.display()))?;
            src.seek(SeekFrom::Start(entry.offset)).map_err(|e| e.to_string())?;
            let mut hash = Context::new(&SHA256);
            let mut left = entry.len;
            while left > 0 {
                let n = left.min(CHUNK as u64) as usize;
                src.read_exact(&mut buf[..n]).map_err(|_| damaged)?;
                hash.update(&buf[..n]);
                out.write_all(&buf[..n]).map_err(|e| format!("No se pudo escribir {} ({e}).", temp.display()))?;
                left -= n as u64;
                done += n as u64;
                progress(done, &entry.path);
            }
            drop(out);
            let hex: String = hash.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
            if !hex.eq_ignore_ascii_case(&entry.sha256) {
                let _ = std::fs::remove_file(&temp);
                return Err(damaged.into());
            }
            replace(&temp, &target)?;
        }
        Ok(())
    }

    /// Escribe el desinstalador: este programa sin los archivos pegados.
    pub fn write_uninstaller(&self, to: &Path) -> Result<(), String> {
        copy_prefix(&self.exe, self.index.stub_len, to)
    }
}

/// Copia los primeros `len` bytes de `from` en `to`.
pub fn copy_prefix(from: &Path, len: u64, to: &Path) -> Result<(), String> {
    let temp = PathBuf::from(format!("{}.nuevo", to.display()));
    let mut src = File::open(from).map_err(|e| e.to_string())?.take(len);
    let mut out = File::create(&temp).map_err(|e| format!("No se pudo escribir {} ({e}).", temp.display()))?;
    std::io::copy(&mut src, &mut out).map_err(|e| e.to_string())?;
    drop(out);
    replace(&temp, to)
}

/// Pone `temp` en lugar de `target`. Si el archivo esta en uso (el navegador
/// abierto), Windows no deja reemplazarlo: se reintenta un momento.
fn replace(temp: &Path, target: &Path) -> Result<(), String> {
    for attempt in 0.. {
        match std::fs::rename(temp, target) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < 15 => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(e) => {
                let _ = std::fs::remove_file(temp);
                return Err(format!(
                    "No se pudo reemplazar {} ({e}). Cierra Michi y vuelve a intentarlo.",
                    target.display()
                ));
            }
        }
    }
    unreachable!()
}

/// Una ruta del indice, solo si es relativa y no sale de la carpeta.
fn safe_relative(path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    let ok = !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then(|| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_paths_cannot_escape_the_install_folder() {
        assert!(safe_relative("michi.exe").is_some());
        assert!(safe_relative("wallpapers/torii-carmesi.jpg").is_some());
        assert!(safe_relative("../evil.exe").is_none());
        assert!(safe_relative("C:\\Windows\\evil.exe").is_none());
        assert!(safe_relative("\\evil.exe").is_none());
        assert!(safe_relative("").is_none());
    }
}
