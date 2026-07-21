use std::sync::Arc;

use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::path::Path as StorePath;
use object_store::{ObjectStore, ObjectStoreExt};

use crate::config::{Config, StorageConfig};

/// Key HEADed by the health probe. It never needs to exist — a `NotFound`
/// answer still proves the store is reachable and answering.
const HEALTH_PROBE_KEY: &str = "_healthz/probe";

/// Builds the object store described by `config`: a local directory for
/// self-host, or an S3-compatible endpoint (MinIO, R2, AWS) for the hosted
/// instance.
pub fn build_object_store(config: &Config) -> anyhow::Result<Arc<dyn ObjectStore>> {
    match &config.storage {
        StorageConfig::Fs { root } => {
            // LocalFileSystem refuses a missing root, so materialize it first.
            std::fs::create_dir_all(root)?;
            let store = LocalFileSystem::new_with_prefix(root)?;
            Ok(Arc::new(store))
        }
        StorageConfig::S3 {
            endpoint,
            bucket,
            region,
            access_key_id,
            secret_access_key,
            allow_http,
        } => {
            let mut builder = AmazonS3Builder::new()
                .with_bucket_name(bucket)
                .with_region(region)
                .with_allow_http(*allow_http);
            if let Some(endpoint) = endpoint {
                builder = builder.with_endpoint(endpoint);
            }
            if let Some(key) = access_key_id {
                builder = builder.with_access_key_id(key);
            }
            if let Some(secret) = secret_access_key {
                builder = builder.with_secret_access_key(secret);
            }
            Ok(Arc::new(builder.build()?))
        }
    }
}

/// HEADs a well-known key to prove the store is reachable. `NotFound` is a
/// healthy answer (the store responded); any other error is a real failure.
pub async fn health_probe(store: &dyn ObjectStore) -> Result<(), object_store::Error> {
    match store.head(&StorePath::from(HEALTH_PROBE_KEY)).await {
        Ok(_) => Ok(()),
        Err(object_store::Error::NotFound { .. }) => Ok(()),
        Err(e) => Err(e),
    }
}
