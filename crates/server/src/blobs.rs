//! Content-addressed blob helpers shared by the math API handlers and the stats
//! task: object-store key layout, lowercase-hex encode/decode, and small-file
//! fetch.
//!
//! Blob bytes live at `blobs/<workspace_uuid>/<hex sha256>`. The key is
//! workspace-prefixed so a member authenticated for one workspace can never
//! reach another workspace's bytes, and so dedup stays workspace-scoped.

use object_store::path::Path as StorePath;
use object_store::{ObjectStore, ObjectStoreExt};
use uuid::Uuid;

/// Object-store key for a blob: `blobs/<workspace>/<hex sha256>`.
pub fn blob_key(workspace_id: Uuid, hash_hex: &str) -> StorePath {
    StorePath::from(format!("blobs/{workspace_id}/{hash_hex}"))
}

/// Lowercase-hex encode bytes (used to move a `BYTEA` hash onto the wire).
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // `from_digit(_, 16)` yields lowercase hex digits.
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    out
}

/// Decode a hex string to bytes, or `None` if it is not valid hex.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

/// True for exactly 64 lowercase hex chars — the on-the-wire form of a sha256.
pub fn is_hex64_lower(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Read a whole blob into memory. Only used for the small files the stats task
/// materializes (`index.json`, lookup CSVs) — never for books.
pub async fn fetch_blob_vec(
    store: &dyn ObjectStore,
    workspace_id: Uuid,
    hash_hex: &str,
) -> object_store::Result<Vec<u8>> {
    let key = blob_key(workspace_id, hash_hex);
    Ok(store.get(&key).await?.bytes().await?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips() {
        let bytes = [0x00u8, 0x0f, 0xa1, 0xff, 0x10];
        assert_eq!(to_hex(&bytes), "000fa1ff10");
        assert_eq!(from_hex("000fa1ff10").unwrap(), bytes);
        assert_eq!(
            from_hex(&to_hex(&[0xde, 0xad, 0xbe, 0xef])).unwrap(),
            [0xde, 0xad, 0xbe, 0xef]
        );
    }

    #[test]
    fn from_hex_rejects_malformed() {
        assert!(from_hex("abc").is_none()); // odd length
        assert!(from_hex("zz").is_none()); // non-hex
    }

    #[test]
    fn hex64_lower_rules() {
        let ok = "a".repeat(64);
        assert!(is_hex64_lower(&ok));
        assert!(!is_hex64_lower(&"A".repeat(64))); // uppercase rejected
        assert!(!is_hex64_lower(&"a".repeat(63))); // wrong length
        assert!(!is_hex64_lower(&"g".repeat(64))); // non-hex
    }

    #[test]
    fn key_is_workspace_prefixed() {
        let ws = Uuid::nil();
        let key = blob_key(ws, "abcd");
        assert_eq!(
            key.as_ref(),
            "blobs/00000000-0000-0000-0000-000000000000/abcd"
        );
    }
}
