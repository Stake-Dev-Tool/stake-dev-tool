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

/// Garbage-collect a workspace's now-unreferenced blobs, returning the freed
/// `(sha256 bytes, size)` pairs so the caller can delete the store objects.
///
/// A blob is orphaned when NO `revision_files` row of ANY revision in the
/// workspace references it, AND NO `front_bundles` manifest of the workspace
/// references it (manifests are `{ "<path>": { "hash": "<hex>", ... } }` JSONB —
/// hashes are extracted with `jsonb_each` and `decode(..., 'hex')`). The whole
/// query is scoped to `workspace_id`, so a byte-identical blob in another
/// workspace is a different `(workspace_id, hash)` row (and a different store
/// key) and is never touched.
///
/// Runs the DELETE on `conn` — call it INSIDE the same transaction that removed
/// the revision/bundle rows (and AFTER that removal), so the orphan set already
/// reflects the deletion.
pub(crate) async fn gc_orphaned_blobs(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<(Vec<u8>, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (Vec<u8>, i64)>(
        "DELETE FROM blobs b \
         WHERE b.workspace_id = $1 \
           AND NOT EXISTS ( \
             SELECT 1 FROM revision_files rf \
             JOIN revisions r ON r.id = rf.revision_id \
             JOIN games g ON g.id = r.game_id \
             WHERE g.workspace_id = $1 AND rf.hash = b.hash) \
           AND NOT EXISTS ( \
             SELECT 1 FROM front_bundles fb \
             JOIN games g2 ON g2.id = fb.game_id \
             CROSS JOIN LATERAL jsonb_each(fb.manifest) AS m(key, val) \
             WHERE g2.workspace_id = $1 AND decode(m.val ->> 'hash', 'hex') = b.hash) \
         RETURNING b.hash, b.size",
    )
    .bind(workspace_id)
    .fetch_all(conn)
    .await
}

/// Best-effort delete of the object-store bytes for freed blobs. Failures are
/// logged, never surfaced — the DB rows are already gone, so orphaned bytes are
/// harmless (and the object may already be absent).
pub(crate) async fn delete_blob_objects(
    store: &dyn ObjectStore,
    workspace_id: Uuid,
    freed: &[(Vec<u8>, i64)],
) {
    for (hash, _) in freed {
        let key = blob_key(workspace_id, &to_hex(hash));
        if let Err(e) = store.delete(&key).await {
            tracing::warn!(error = %e, key = %key, "blob GC: store delete failed");
        }
    }
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
