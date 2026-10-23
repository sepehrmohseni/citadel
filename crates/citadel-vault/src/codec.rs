use std::io::{self, Read, Write};

use ruzstd::decoding::StreamingDecoder;
use ruzstd::encoding::{CompressionLevel, compress};
use serde::Serialize;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use crate::{Result, VaultError};

pub(crate) const MAX_PLAIN: usize = 64 << 20;

struct Tally(usize);

impl Write for Tally {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn encode<T: Serialize + ?Sized>(val: &T) -> Result<Zeroizing<Vec<u8>>> {
    let mut tally = Tally(0);
    rmp_serde::encode::write_named(&mut tally, val)?;
    let mut buf = Zeroizing::new(Vec::with_capacity(tally.0));
    rmp_serde::encode::write_named(&mut *buf, val)?;
    Ok(buf)
}

pub(crate) fn decode<T: DeserializeOwned>(raw: &[u8]) -> Result<T> {
    Ok(rmp_serde::from_slice(raw)?)
}

pub(crate) fn pack<T: Serialize + ?Sized>(val: &T) -> Result<Zeroizing<Vec<u8>>> {
    let raw = encode(val)?;
    if raw.len() > MAX_PLAIN {
        return Err(VaultError::Oversize(raw.len() as u64));
    }
    let len = u32::try_from(raw.len()).map_err(|_| VaultError::Oversize(raw.len() as u64))?;
    let bound = raw.len() + (raw.len() >> 7) + 1024;
    let mut out = Zeroizing::new(Vec::with_capacity(4 + bound));
    out.extend_from_slice(&len.to_le_bytes());
    compress(raw.as_slice(), &mut *out, CompressionLevel::Fastest);
    Ok(out)
}

pub(crate) fn unpack<T: DeserializeOwned>(packed: &[u8]) -> Result<T> {
    let (len, frame) = packed
        .split_first_chunk::<4>()
        .ok_or(VaultError::Truncated)?;
    let len = u32::from_le_bytes(*len) as usize;
    if len > MAX_PLAIN {
        return Err(VaultError::Oversize(len as u64));
    }
    let mut dec = StreamingDecoder::new(frame).map_err(|e| VaultError::Codec(e.to_string()))?;
    let mut raw = Zeroizing::new(vec![0u8; len]);
    dec.read_exact(raw.as_mut_slice())
        .map_err(|e| VaultError::Codec(e.to_string()))?;
    let mut probe = [0u8; 1];
    let trailing = dec
        .read(&mut probe)
        .map_err(|e| VaultError::Codec(e.to_string()))?;
    if trailing != 0 {
        return Err(VaultError::Codec(
            "frame longer than declared length".into(),
        ));
    }
    decode(&raw)
}
