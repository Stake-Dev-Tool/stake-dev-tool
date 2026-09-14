//! Original build files, streamed without recompression or whole-blob buffering.
use axum::body::{Body, Bytes};
use axum::http::header;
use axum::response::Response;
use futures_util::StreamExt;
use object_store::ObjectStoreExt;
use protocol::FileEntry;
use uuid::Uuid;

use crate::{
    AppState, blobs,
    error::{ApiError, ApiResult},
};

pub(super) fn download(
    state: AppState,
    workspace_id: Uuid,
    files: Vec<FileEntry>,
    filename: String,
) -> ApiResult<Response> {
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        // Validate again at the export boundary, including legacy/corrupt rows.
        // Colons exclude Windows drive paths and alternate data streams.
        if file.size < 0 || !blobs::is_hex64_lower(&file.hash) {
            return Err(ApiError::unprocessable(
                "invalid_manifest",
                "invalid archive file metadata",
            ));
        }
        if file.path.len() > 512
            || file.path.contains(['\\', ':'])
            || file.path.chars().any(char::is_control)
            || file
                .path
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(ApiError::unprocessable(
                "invalid_manifest",
                "unsafe archive path",
            ));
        }
        let mut prefix = Vec::new();
        let mut header = tar::Header::new_gnu();
        if file.path.len() > 100 {
            // GNU long-name extension carries the complete UTF-8 path, including
            // paths too long for ustar's prefix/name split. Only metadata is buffered.
            let mut long = tar::Header::new_gnu();
            long.set_path("././@LongLink").map_err(ApiError::internal)?;
            long.set_entry_type(tar::EntryType::GNULongName);
            long.set_size(file.path.len() as u64 + 1);
            long.set_mode(0o644);
            long.set_cksum();
            prefix.extend_from_slice(long.as_bytes());
            prefix.extend_from_slice(file.path.as_bytes());
            prefix.push(0);
            prefix.resize(prefix.len().next_multiple_of(512), 0);
            header.set_path("file").map_err(ApiError::internal)?;
        } else {
            header.set_path(&file.path).map_err(ApiError::internal)?;
        }
        header.set_size(file.size as u64);
        header.set_mode(0o644);
        header.set_cksum();
        prefix.extend_from_slice(header.as_bytes());
        entries.push((file, Bytes::from(prefix)));
    }
    // The HTTP consumer drives this stream: at most one object-store chunk and
    // small tar headers are held, and dropping the body cancels further reads.
    let stream = async_stream::try_stream! {
        for (file, header) in entries {
            let object = state.store.get(&blobs::blob_key(workspace_id, &file.hash)).await
                .map_err(std::io::Error::other)?;
            if object.meta.size != file.size as u64 {
                Err(std::io::Error::other("archive object size differs from manifest"))?;
            }
            yield header;
            let mut chunks = object.into_stream();
            let mut remaining = file.size as u64;
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(std::io::Error::other)?;
                remaining = remaining.checked_sub(chunk.len() as u64)
                    .ok_or_else(|| std::io::Error::other("archive object exceeds declared size"))?;
                yield chunk;
            }
            if remaining != 0 {
                Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "archive object is truncated"))?;
            }
            let padding = (512 - file.size as usize % 512) % 512;
            if padding != 0 { yield Bytes::from(vec![0; padding]); }
        }
        yield Bytes::from_static(&[0; 1024]);
    };
    let stream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
    > = Box::pin(stream);
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/x-tar"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}
