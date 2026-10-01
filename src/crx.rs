//! Verificacion de paquetes de extension .crx (formato CRX3), como hace
//! Chrome antes de instalar uno.
//!
//! Un .crx es: "Cr24", la version (3), el largo del encabezado, el
//! encabezado (un protobuf CrxFileHeader con una o mas pruebas de firma) y
//! el .zip de la extension. Cada prueba es una clave publica (RSA o ECDSA
//! P-256) y la firma, con esa clave, de:
//!
//!   "CRX3 SignedData\0" + largo (u32 LE) de signed_header_data
//!     + signed_header_data + el .zip
//!
//! signed_header_data trae el id de la extension (16 bytes), y el id de una
//! extension ES el SHA-256 de la clave de su desarrollador (los primeros 16
//! bytes, escritos con las letras a-p). Asi que si todas las firmas son
//! validas y una de las claves corresponde al id, el .zip es exactamente el
//! que publico el dueno de ese id: nadie puede cambiarle un byte (ni quien se
//! ponga en el medio de la descarga, ni un .crx modificado que circule por
//! ahi) sin su clave privada.
//!
//! El formato viejo (CRX2, firmado con SHA-1) se rechaza, como en Chrome.

use ring::digest::{digest, SHA256};
use ring::signature::{UnparsedPublicKey, VerificationAlgorithm, ECDSA_P256_SHA256_ASN1, RSA_PKCS1_2048_8192_SHA256};

const MAGIC: &[u8] = b"Cr24";
/// Lo que se antepone a los datos firmados (con su \0 final, como en Chromium).
const SIGNATURE_CONTEXT: &[u8] = b"CRX3 SignedData\x00";
/// Un encabezado real ocupa unos pocos KB: esto es solo un tope de cordura.
const MAX_HEADER: usize = 1 << 20;

/// Un .crx cuya firma ya se verifico.
#[derive(Debug)]
pub struct Package<'a> {
    /// Id de la extension (32 letras a-p) que garantiza la firma.
    pub id: String,
    /// El .zip con los archivos de la extension.
    pub archive: &'a [u8],
}

pub fn is_crx(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

pub fn verify(bytes: &[u8]) -> Result<Package<'_>, String> {
    let damaged = || "Archivo .crx danado.".to_string();
    let invalid = || "La firma del .crx no es valida: el archivo fue modificado o no es el original.".to_string();

    if !is_crx(bytes) {
        return Err("No es un archivo .crx.".into());
    }
    let version = u32_le(bytes, 4).ok_or_else(damaged)?;
    if version != 3 {
        return Err(format!(
            "Formato .crx no soportado (version {version}): solo se aceptan paquetes CRX3 firmados."
        ));
    }
    let header_len = u32_le(bytes, 8).ok_or_else(damaged)? as usize;
    if header_len > MAX_HEADER {
        return Err(damaged());
    }
    let header = bytes.get(12..12 + header_len).ok_or_else(damaged)?;
    let archive = &bytes[12 + header_len..];

    let mut rsa = Vec::new();
    let mut ecdsa = Vec::new();
    let mut signed_data = None;
    for (number, value) in fields(header).ok_or_else(damaged)? {
        match number {
            2 => rsa.push(proof(value).ok_or_else(damaged)?),
            3 => ecdsa.push(proof(value).ok_or_else(damaged)?),
            10000 => signed_data = Some(value),
            _ => {}
        }
    }
    let signed_data = signed_data.ok_or("El .crx no declara el id de la extension.")?;
    let crx_id = fields(signed_data)
        .ok_or_else(damaged)?
        .into_iter()
        .find(|(number, _)| *number == 1)
        .map(|(_, value)| value)
        .filter(|id| id.len() == 16)
        .ok_or_else(damaged)?;
    if rsa.is_empty() && ecdsa.is_empty() {
        return Err("El .crx no esta firmado.".into());
    }

    let mut message = Vec::with_capacity(SIGNATURE_CONTEXT.len() + 4 + signed_data.len() + archive.len());
    message.extend_from_slice(SIGNATURE_CONTEXT);
    message.extend_from_slice(&(signed_data.len() as u32).to_le_bytes());
    message.extend_from_slice(signed_data);
    message.extend_from_slice(archive);

    // Todas las firmas tienen que ser validas (no alcanza con una).
    let algorithms = rsa
        .iter()
        .map(|p| (p, &RSA_PKCS1_2048_8192_SHA256 as &'static dyn VerificationAlgorithm))
        .chain(ecdsa.iter().map(|p| (p, &ECDSA_P256_SHA256_ASN1 as &'static dyn VerificationAlgorithm)));
    for (proof, algorithm) in algorithms {
        let key = spki_key(proof.key).ok_or_else(invalid)?;
        UnparsedPublicKey::new(algorithm, key)
            .verify(&message, proof.signature)
            .map_err(|_| invalid())?;
    }

    // Y una de las claves tiene que ser la que da el id: si no, cualquiera
    // podria firmar un .zip propio con su clave y ponerle el id de otro.
    let developer_key = rsa
        .iter()
        .chain(&ecdsa)
        .any(|p| digest(&SHA256, p.key).as_ref()[..16] == *crx_id);
    if !developer_key {
        return Err("La firma del .crx no corresponde a la extension que dice ser.".into());
    }
    Ok(Package { id: encode_id(crx_id), archive })
}

struct Proof<'a> {
    /// Clave publica X.509 (SubjectPublicKeyInfo, DER).
    key: &'a [u8],
    signature: &'a [u8],
}

/// AsymmetricKeyProof { public_key = 1; signature = 2; }
fn proof(message: &[u8]) -> Option<Proof<'_>> {
    let (mut key, mut signature) = (None, None);
    for (number, value) in fields(message)? {
        match number {
            1 => key = Some(value),
            2 => signature = Some(value),
            _ => {}
        }
    }
    Some(Proof { key: key?, signature: signature? })
}

/// Id de extension: cada medio byte como una letra de la 'a' a la 'p'.
fn encode_id(raw: &[u8]) -> String {
    raw.iter().flat_map(|b| [b >> 4, b & 0x0f]).map(|n| (b'a' + n) as char).collect()
}

fn u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Los campos "length-delimited" (los unicos que usa CRX3) de un mensaje
/// protobuf, como (numero de campo, bytes). Los de otros tipos se saltean.
/// None si el mensaje esta mal formado.
fn fields(mut buf: &[u8]) -> Option<Vec<(u64, &[u8])>> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let key = varint(&mut buf)?;
        match key & 7 {
            0 => {
                varint(&mut buf)?;
            }
            1 => buf = buf.get(8..)?,
            2 => {
                let len = usize::try_from(varint(&mut buf)?).ok()?;
                let value = buf.get(..len)?;
                buf = &buf[len..];
                out.push((key >> 3, value));
            }
            5 => buf = buf.get(4..)?,
            _ => return None,
        }
    }
    Some(out)
}

fn varint(buf: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for i in 0..10 {
        let (&byte, rest) = buf.split_first()?;
        *buf = rest;
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// De una clave publica X.509 (SubjectPublicKeyInfo, DER) devuelve la clave
/// en si, el contenido del BIT STRING: RSAPublicKey para RSA, el punto de la
/// curva para ECDSA, que es lo que espera ring.
fn spki_key(spki: &[u8]) -> Option<&[u8]> {
    let (info, rest) = der(spki, 0x30)?;
    if !rest.is_empty() {
        return None;
    }
    let (_algorithm, rest) = der(info, 0x30)?;
    let (bits, rest) = der(rest, 0x03)?;
    if !rest.is_empty() {
        return None;
    }
    match bits.split_first()? {
        (0, key) => Some(key),
        _ => None,
    }
}

/// Un elemento DER con la etiqueta esperada: (contenido, lo que le sigue).
fn der(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&found, rest) = input.split_first()?;
    if found != tag {
        return None;
    }
    let (&first, mut rest) = rest.split_first()?;
    let len = if first < 0x80 {
        first as usize
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let bytes = rest.get(..n)?;
        rest = &rest[n..];
        bytes.iter().fold(0usize, |acc, b| (acc << 8) | *b as usize)
    };
    let content = rest.get(..len)?;
    Some((content, &rest[len..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};

    /// Encabezado DER de una clave publica P-256: va antes del punto (65 bytes).
    const P256_SPKI_PREFIX: &[u8] = &[
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86,
        0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];

    fn put_varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn put_bytes(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
        put_varint(out, (field << 3) | 2);
        put_varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    /// Arma un .crx firmado con una clave ECDSA nueva. `declared` reemplaza
    /// el id que corresponde a la clave (para probar un id ajeno).
    fn build(archive: &[u8], declared: Option<[u8; 16]>) -> (Vec<u8>, String) {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).unwrap();
        let spki = [P256_SPKI_PREFIX, key.public_key().as_ref()].concat();
        let real_id: [u8; 16] = digest(&SHA256, &spki).as_ref()[..16].try_into().unwrap();
        let crx_id = declared.unwrap_or(real_id);

        let mut signed_data = Vec::new();
        put_bytes(&mut signed_data, 1, &crx_id);
        let message = [
            SIGNATURE_CONTEXT,
            &(signed_data.len() as u32).to_le_bytes(),
            &signed_data,
            archive,
        ]
        .concat();
        let signature = key.sign(&rng, &message).unwrap();

        let mut proof = Vec::new();
        put_bytes(&mut proof, 1, &spki);
        put_bytes(&mut proof, 2, signature.as_ref());
        let mut header = Vec::new();
        put_bytes(&mut header, 3, &proof);
        put_bytes(&mut header, 10000, &signed_data);

        let mut crx = b"Cr24".to_vec();
        crx.extend(3u32.to_le_bytes());
        crx.extend((header.len() as u32).to_le_bytes());
        crx.extend(&header);
        crx.extend(archive);
        (crx, encode_id(&crx_id))
    }

    #[test]
    fn accepts_a_correctly_signed_package() {
        let (crx, id) = build(b"PK\x03\x04 contenido", None);
        let package = verify(&crx).unwrap();
        assert_eq!(package.id, id);
        assert_eq!(package.id.len(), 32);
        assert_eq!(package.archive, b"PK\x03\x04 contenido");
    }

    #[test]
    fn rejects_a_modified_archive() {
        let (mut crx, _) = build(b"PK\x03\x04 contenido", None);
        let last = crx.len() - 1;
        crx[last] ^= 1;
        assert!(verify(&crx).is_err());
    }

    #[test]
    fn rejects_a_signature_from_a_key_that_is_not_the_extension_id() {
        let (crx, _) = build(b"PK\x03\x04 contenido", Some([7; 16]));
        assert!(verify(&crx).unwrap_err().contains("no corresponde"));
    }

    #[test]
    fn rejects_unsigned_and_old_formats() {
        let mut crx2 = b"Cr24".to_vec();
        crx2.extend(2u32.to_le_bytes());
        crx2.extend([0; 8]);
        assert!(verify(&crx2).unwrap_err().contains("version 2"));
        assert!(verify(b"PK\x03\x04").is_err());
        assert!(verify(b"Cr24").is_err());

        let mut no_proofs = b"Cr24".to_vec();
        no_proofs.extend(3u32.to_le_bytes());
        let mut header = Vec::new();
        let mut signed_data = Vec::new();
        put_bytes(&mut signed_data, 1, &[1; 16]);
        put_bytes(&mut header, 10000, &signed_data);
        no_proofs.extend((header.len() as u32).to_le_bytes());
        no_proofs.extend(header);
        assert!(verify(&no_proofs).unwrap_err().contains("no esta firmado"));
    }

    #[test]
    fn ids_use_letters_a_to_p() {
        assert_eq!(encode_id(&[0x00, 0x1f, 0xff]), "aabppp");
    }
}
