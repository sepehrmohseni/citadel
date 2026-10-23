use argon2::{Algorithm, Argon2, Block, Params, Version};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{Result, VaultError};

pub(crate) const KEY_LEN: usize = 32;
pub(crate) const SALT_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    pub const RECOMMENDED: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 4,
    };
    pub const MAX_M_KIB: u32 = 1024 * 1024;
    pub const MAX_T: u32 = 16;
    pub const MAX_P: u32 = 16;

    pub fn validate(self) -> Result<Self> {
        let Self { m_kib, t, p } = self;
        let sane = (1..=Self::MAX_P).contains(&p)
            && (1..=Self::MAX_T).contains(&t)
            && m_kib >= 8 * p
            && m_kib <= Self::MAX_M_KIB;
        if sane {
            Ok(self)
        } else {
            Err(VaultError::KdfBounds { m_kib, t, p })
        }
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::RECOMMENDED
    }
}

pub(crate) fn derive(
    password: &SecretString,
    salt: &[u8; SALT_LEN],
    kdf: KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let kdf = kdf.validate()?;
    let params = Params::new(kdf.m_kib, kdf.t, kdf.p, Some(KEY_LEN)).map_err(VaultError::Kdf)?;
    let blocks = params.block_count();
    let engine = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut arena = Zeroizing::new(vec![Block::default(); blocks]);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    engine
        .hash_password_into_with_memory(
            password.expose_secret().as_bytes(),
            salt,
            out.as_mut_slice(),
            arena.as_mut_slice(),
        )
        .map_err(VaultError::Kdf)?;
    Ok(out)
}
