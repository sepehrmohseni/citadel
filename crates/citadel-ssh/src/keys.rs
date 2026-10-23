use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, EcdsaCurve, PrivateKey};
use secrecy::SecretString;
use serde::Serialize;

use crate::{Result, auth};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    Ed25519,
    EcdsaP256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KeyInfo {
    pub algorithm: String,
    pub public_key: String,
    pub fingerprint: String,
}

pub fn inspect(key: &PrivateKey) -> Result<KeyInfo> {
    let public = key.public_key();
    Ok(KeyInfo {
        algorithm: public.algorithm().to_string(),
        public_key: public.to_openssh()?,
        fingerprint: auth::fingerprint(public),
    })
}

pub fn generate(kind: KeyKind, comment: &str) -> Result<(SecretString, KeyInfo)> {
    let alg = match kind {
        KeyKind::Ed25519 => Algorithm::Ed25519,
        KeyKind::EcdsaP256 => Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        },
    };
    let mut key = PrivateKey::random(&mut rand::rng(), alg)?;
    key.set_comment(comment);
    let pem = key.to_openssh(LineEnding::LF)?;
    Ok((SecretString::from(pem.as_str()), inspect(&key)?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyFamily {
    Ed25519,
    Ecdsa,
    Rsa,
    Dsa,
    Other,
}

pub fn classify(public_openssh: &str) -> Result<(KeyFamily, u32)> {
    use russh::keys::ssh_key::public::KeyData;
    let key = russh::keys::PublicKey::from_openssh(public_openssh.trim())?;
    Ok(match key.key_data() {
        KeyData::Ed25519(_) => (KeyFamily::Ed25519, 256),
        KeyData::Ecdsa(e) => (
            KeyFamily::Ecdsa,
            match e.curve() {
                EcdsaCurve::NistP256 => 256,
                EcdsaCurve::NistP384 => 384,
                EcdsaCurve::NistP521 => 521,
            },
        ),
        KeyData::Rsa(r) => (KeyFamily::Rsa, r.key_size()),
        KeyData::Dsa(_) => (KeyFamily::Dsa, 1024),
        _ => (KeyFamily::Other, 0),
    })
}
