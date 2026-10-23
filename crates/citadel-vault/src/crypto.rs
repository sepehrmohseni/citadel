use chacha20poly1305::{AeadInOut, Key, KeyInit, Tag, XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::kdf::KEY_LEN;
use crate::{Result, VaultError};

pub(crate) const NONCE_LEN: usize = 24;
pub(crate) const TAG_LEN: usize = 16;

pub(crate) fn cipher(key: &Zeroizing<[u8; KEY_LEN]>) -> XChaCha20Poly1305 {
    let key: &Key = (&**key).into();
    XChaCha20Poly1305::new(key)
}

pub(crate) fn nonce() -> Result<[u8; NONCE_LEN]> {
    let mut n = [0u8; NONCE_LEN];
    getrandom::fill(&mut n)?;
    Ok(n)
}

pub(crate) fn random_key() -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::fill(k.as_mut_slice())?;
    Ok(k)
}

pub(crate) fn encrypt(
    cipher: &XChaCha20Poly1305,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    buf: &mut [u8],
) -> Result<[u8; TAG_LEN]> {
    let len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
    let nonce: &XNonce = nonce.into();
    let tag = cipher
        .encrypt_inout_detached(nonce, aad, buf.into())
        .map_err(|_| VaultError::Oversize(len))?;
    Ok(tag.into())
}

pub(crate) fn decrypt(
    cipher: &XChaCha20Poly1305,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    buf: &mut [u8],
    tag: &[u8],
) -> Result<()> {
    let tag = Tag::try_from(tag).map_err(|_| VaultError::Truncated)?;
    let nonce: &XNonce = nonce.into();
    cipher
        .decrypt_inout_detached(nonce, aad, buf.into(), &tag)
        .map_err(|_| VaultError::Tampered)
}

pub(crate) fn seal(cipher: &XChaCha20Poly1305, aad: &[u8], plain: &[u8]) -> Result<Vec<u8>> {
    let nonce = nonce()?;
    let mut out = Vec::with_capacity(NONCE_LEN + plain.len() + TAG_LEN);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(plain);
    let tag = encrypt(cipher, &nonce, aad, &mut out[NONCE_LEN..])?;
    out.extend_from_slice(&tag);
    Ok(out)
}

pub(crate) fn open(
    cipher: &XChaCha20Poly1305,
    aad: &[u8],
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let body_len = blob
        .len()
        .checked_sub(NONCE_LEN + TAG_LEN)
        .ok_or(VaultError::Truncated)?;
    let (nonce, rest) = blob.split_at(NONCE_LEN);
    let (body, tag) = rest.split_at(body_len);
    let nonce: &[u8; NONCE_LEN] = nonce.try_into().map_err(|_| VaultError::Truncated)?;
    let mut plain = Zeroizing::new(body.to_vec());
    decrypt(cipher, nonce, aad, plain.as_mut_slice(), tag)?;
    Ok(plain)
}
