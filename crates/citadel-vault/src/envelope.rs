use chacha20poly1305::XChaCha20Poly1305;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{self, NONCE_LEN, TAG_LEN};
use crate::kdf::{self, KEY_LEN, KdfParams, SALT_LEN};
use crate::model::Entry;
use crate::{Result, VaultError, codec};

pub const MAGIC: [u8; 4] = *b"CTDL";
pub const FORMAT_VERSION: u8 = 1;

const WRAP_AAD_LEN: usize = 4 + 1 + 12 + 8 + SALT_LEN + NONCE_LEN;
const WRAPPED_LEN: usize = KEY_LEN + TAG_LEN;
pub(crate) const HEADER_LEN: usize = WRAP_AAD_LEN + WRAPPED_LEN;
const PAYLOAD_AAD_LEN: usize = HEADER_LEN + NONCE_LEN;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyHeader {
    pub kdf: KdfParams,
    pub epoch: u64,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
    wrapped: [u8; WRAPPED_LEN],
}

impl KeyHeader {
    pub fn wrap(
        password: &SecretString,
        dek: &Zeroizing<[u8; KEY_LEN]>,
        kdf: KdfParams,
        epoch: u64,
    ) -> Result<Self> {
        let mut salt = [0u8; SALT_LEN];
        getrandom::fill(&mut salt)?;
        let mut hdr = Self {
            kdf: kdf.validate()?,
            epoch,
            salt,
            nonce: crypto::nonce()?,
            wrapped: [0u8; WRAPPED_LEN],
        };
        let kek = crypto::cipher(&kdf::derive(password, &hdr.salt, hdr.kdf)?);
        let mut buf = Zeroizing::new(**dek);
        let aad = hdr.wrap_aad();
        let tag = crypto::encrypt(&kek, &hdr.nonce, &aad, buf.as_mut_slice())?;
        hdr.wrapped[..KEY_LEN].copy_from_slice(buf.as_slice());
        hdr.wrapped[KEY_LEN..].copy_from_slice(&tag);
        Ok(hdr)
    }

    pub fn unwrap_key(&self, password: &SecretString) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        let kek = crypto::cipher(&kdf::derive(password, &self.salt, self.kdf)?);
        let mut dek = Zeroizing::new([0u8; KEY_LEN]);
        dek.copy_from_slice(&self.wrapped[..KEY_LEN]);
        crypto::decrypt(
            &kek,
            &self.nonce,
            &self.wrap_aad(),
            dek.as_mut_slice(),
            &self.wrapped[KEY_LEN..],
        )
        .map_err(|e| match e {
            VaultError::Tampered => VaultError::BadPassword,
            other => other,
        })?;
        Ok(dek)
    }

    fn wrap_aad(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(WRAP_AAD_LEN);
        out.extend_from_slice(&MAGIC);
        out.push(FORMAT_VERSION);
        out.extend_from_slice(&self.kdf.m_kib.to_le_bytes());
        out.extend_from_slice(&self.kdf.t.to_le_bytes());
        out.extend_from_slice(&self.kdf.p.to_le_bytes());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.nonce);
        out
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.wrap_aad();
        out.extend_from_slice(&self.wrapped);
        out
    }

    pub fn decode(raw: &[u8]) -> Result<Self> {
        let mut r = Reader::new(raw);
        if r.take::<4>().map_or(true, |m| m != MAGIC) {
            return Err(if raw.len() < MAGIC.len() {
                VaultError::Truncated
            } else {
                VaultError::BadMagic
            });
        }
        let [version] = r.take::<1>()?;
        if version != FORMAT_VERSION {
            return Err(VaultError::UnsupportedVersion(version));
        }
        let kdf = KdfParams {
            m_kib: r.u32()?,
            t: r.u32()?,
            p: r.u32()?,
        };
        Ok(Self {
            kdf,
            epoch: r.u64()?,
            salt: r.take()?,
            nonce: r.take()?,
            wrapped: r.take()?,
        })
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self.pos.checked_add(N).ok_or(VaultError::Truncated)?;
        let chunk = self.buf.get(self.pos..end).ok_or(VaultError::Truncated)?;
        self.pos = end;
        chunk.try_into().map_err(|_| VaultError::Truncated)
    }

    fn u32(&mut self) -> Result<u32> {
        self.take().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64> {
        self.take().map(u64::from_le_bytes)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Bundle {
    pub created_ms: u64,
    pub entries: Vec<Entry>,
}

impl Bundle {
    pub fn seal(&self, password: &SecretString, kdf: KdfParams) -> Result<Vec<u8>> {
        let dek = crypto::random_key()?;
        let header = KeyHeader::wrap(password, &dek, kdf, self.created_ms)?;
        seal(&crypto::cipher(&dek), &header, self)
    }

    pub fn open(blob: &[u8], password: &SecretString) -> Result<Self> {
        let sealed = Sealed::parse(blob)?;
        let dek = sealed.header.unwrap_key(password)?;
        sealed.open(&crypto::cipher(&dek))
    }
}

pub(crate) fn seal(
    cipher: &XChaCha20Poly1305,
    header: &KeyHeader,
    bundle: &Bundle,
) -> Result<Vec<u8>> {
    let packed = codec::pack(bundle)?;
    let nonce = crypto::nonce()?;
    let mut out = Vec::with_capacity(PAYLOAD_AAD_LEN + packed.len() + TAG_LEN);
    out.extend_from_slice(&header.encode());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&packed);
    let (aad, body) = out.split_at_mut(PAYLOAD_AAD_LEN);
    let tag = crypto::encrypt(cipher, &nonce, aad, body)?;
    out.extend_from_slice(&tag);
    Ok(out)
}

pub(crate) struct Sealed<'a> {
    pub header: KeyHeader,
    aad: &'a [u8],
    nonce: [u8; NONCE_LEN],
    body: &'a [u8],
    tag: &'a [u8],
}

impl<'a> Sealed<'a> {
    pub fn parse(blob: &'a [u8]) -> Result<Self> {
        let header = KeyHeader::decode(blob)?;
        let aad = blob.get(..PAYLOAD_AAD_LEN).ok_or(VaultError::Truncated)?;
        let rest = blob.get(PAYLOAD_AAD_LEN..).ok_or(VaultError::Truncated)?;
        let body_len = rest
            .len()
            .checked_sub(TAG_LEN)
            .ok_or(VaultError::Truncated)?;
        let (body, tag) = rest.split_at(body_len);
        let nonce = aad
            .get(HEADER_LEN..)
            .and_then(|n| <[u8; NONCE_LEN]>::try_from(n).ok())
            .ok_or(VaultError::Truncated)?;
        Ok(Self {
            header,
            aad,
            nonce,
            body,
            tag,
        })
    }

    pub fn open(&self, cipher: &XChaCha20Poly1305) -> Result<Bundle> {
        let mut plain = Zeroizing::new(self.body.to_vec());
        crypto::decrypt(
            cipher,
            &self.nonce,
            self.aad,
            plain.as_mut_slice(),
            self.tag,
        )?;
        codec::unpack(&plain)
    }
}
