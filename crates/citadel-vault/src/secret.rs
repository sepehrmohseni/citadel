use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserializer, Serializer, de};
use zeroize::Zeroizing;

pub fn secret_from(raw: String) -> SecretString {
    let raw = Zeroizing::new(raw);
    SecretString::from(raw.as_str())
}

struct SecretVisitor;

impl de::Visitor<'_> for SecretVisitor {
    type Value = SecretString;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a secret string")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<SecretString, E> {
        Ok(SecretString::from(v))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<SecretString, E> {
        Ok(secret_from(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<SecretString, E> {
        std::str::from_utf8(v)
            .map(SecretString::from)
            .map_err(E::custom)
    }
}

struct OptionalVisitor;

impl<'de> de::Visitor<'de> for OptionalVisitor {
    type Value = Option<SecretString>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an optional secret string")
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_string(SecretVisitor).map(Some)
    }
}

pub(crate) mod plain {
    use super::*;

    pub fn serialize<S: Serializer>(v: &SecretString, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(v.expose_secret())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SecretString, D::Error> {
        d.deserialize_string(SecretVisitor)
    }
}

pub(crate) mod optional {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Option<SecretString>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_some(v.expose_secret()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SecretString>, D::Error> {
        d.deserialize_option(OptionalVisitor)
    }
}
