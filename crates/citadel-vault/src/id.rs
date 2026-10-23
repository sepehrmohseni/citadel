use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};
use zeroize::Zeroize;

use crate::{Result, VaultError};

const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Zeroize)]
pub struct Id([u8; 16]);

impl Id {
    pub fn random() -> Result<Self> {
        let mut raw = [0u8; 16];
        getrandom::fill(&mut raw)?;
        Ok(Self(raw))
    }

    pub const fn from_bytes(raw: [u8; 16]) -> Self {
        Self(raw)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    fn hex(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (pair, b) in out.chunks_exact_mut(2).zip(self.0) {
            pair[0] = HEX[usize::from(b >> 4)];
            pair[1] = HEX[usize::from(b & 0x0f)];
        }
        out
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hex = self.hex();
        f.write_str(std::str::from_utf8(&hex).map_err(|_| fmt::Error)?)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({self})")
    }
}

impl FromStr for Id {
    type Err = VaultError;

    fn from_str(s: &str) -> Result<Self> {
        let mut raw = [0u8; 16];
        hex::decode_to_slice(s, &mut raw).map_err(|_| VaultError::BadId)?;
        Ok(Self(raw))
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let hex = self.hex();
        s.serialize_str(std::str::from_utf8(&hex).map_err(ser::Error::custom)?)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct IdVisitor;

        impl de::Visitor<'_> for IdVisitor {
            type Value = Id;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a 32-character hex identifier")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Id, E> {
                v.parse()
                    .map_err(|_| E::invalid_value(de::Unexpected::Str(v), &self))
            }
        }

        d.deserialize_str(IdVisitor)
    }
}
