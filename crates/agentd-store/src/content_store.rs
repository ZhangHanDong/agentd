//! Local content-addressed bytes used by the enterprise artifact adapter.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum ContentStoreError {
    #[error("content store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("content hash mismatch")]
    HashMismatch,
    #[error("invalid content hash")]
    InvalidHash,
    #[error("object store request failed: {0}")]
    Http(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredContent {
    pub sha256: String,
    pub size_bytes: u64,
    pub storage_ref: String,
}

/// Object-store boundary used by artifact producers. The daemon depends on
/// this contract rather than on a filesystem or cloud SDK implementation.
pub trait ArtifactObjectStore: Send + Sync {
    fn put_bytes(&self, bytes: &[u8]) -> Result<StoredContent, ContentStoreError>;
    fn put_file(&self, source: &std::path::Path) -> Result<StoredContent, ContentStoreError>;
    fn get_ref(&self, storage_ref: &str) -> Result<Option<Vec<u8>>, ContentStoreError>;
}

/// S3/MinIO-compatible object adapter using an endpoint that accepts
/// `/<sha256>` object paths. A reverse proxy or bucket gateway can expose this
/// narrow contract without coupling the daemon to a vendor SDK.
#[derive(Debug, Clone)]
pub struct S3CompatibleObjectStore {
    client: reqwest::blocking::Client,
    endpoint: String,
    bearer_token: Option<String>,
}

impl S3CompatibleObjectStore {
    pub fn new(
        endpoint: impl Into<String>,
        bearer_token: Option<String>,
    ) -> Result<Self, ContentStoreError> {
        let endpoint = endpoint.into().trim_end_matches('/').to_owned();
        if endpoint.is_empty() {
            return Err(ContentStoreError::Http(
                "empty object store endpoint".into(),
            ));
        }
        if !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
            return Err(ContentStoreError::Http(
                "object store endpoint must use http:// or https://".into(),
            ));
        }
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|error| ContentStoreError::Http(error.to_string()))?;
        Ok(Self {
            client,
            endpoint,
            bearer_token,
        })
    }

    fn object_url(&self, sha256: &str) -> Result<String, ContentStoreError> {
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ContentStoreError::InvalidHash);
        }
        Ok(format!("{}/{sha256}", self.endpoint))
    }

    fn request(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        match &self.bearer_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    fn put(&self, bytes: &[u8]) -> Result<StoredContent, ContentStoreError> {
        let sha256 = hex_digest(bytes);
        let url = self.object_url(&sha256)?;
        let mut last_status = None;
        for attempt in 0..3 {
            let response = self
                .request(self.client.put(&url).body(bytes.to_vec()))
                .send()
                .map_err(|error| ContentStoreError::Http(error.to_string()))?;
            if response.status().is_success() {
                return Ok(StoredContent {
                    sha256: sha256.clone(),
                    size_bytes: bytes.len() as u64,
                    storage_ref: format!("s3://{sha256}"),
                });
            }
            last_status = Some(response.status());
            if !is_retryable(response.status()) || attempt == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
        }
        Err(ContentStoreError::Http(format!(
            "PUT returned {}",
            last_status.expect("request loop records a status")
        )))
    }
}

impl ArtifactObjectStore for S3CompatibleObjectStore {
    fn put_bytes(&self, bytes: &[u8]) -> Result<StoredContent, ContentStoreError> {
        self.put(bytes)
    }

    fn put_file(&self, source: &std::path::Path) -> Result<StoredContent, ContentStoreError> {
        self.put(&std::fs::read(source)?)
    }

    fn get_ref(&self, storage_ref: &str) -> Result<Option<Vec<u8>>, ContentStoreError> {
        let sha256 = storage_ref
            .strip_prefix("s3://")
            .ok_or(ContentStoreError::InvalidHash)?;
        let url = self.object_url(sha256)?;
        let mut response = None;
        for attempt in 0..3 {
            let current = self
                .request(self.client.get(&url))
                .send()
                .map_err(|error| ContentStoreError::Http(error.to_string()))?;
            if !is_retryable(current.status()) || attempt == 2 {
                response = Some(current);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
        }
        let response = response.expect("request loop returns a response");
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(ContentStoreError::Http(format!(
                "GET returned {}",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .map_err(|error| ContentStoreError::Http(error.to_string()))?
            .to_vec();
        if hex_digest(&bytes) != sha256 {
            return Err(ContentStoreError::HashMismatch);
        }
        Ok(Some(bytes))
    }
}

fn is_retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

#[derive(Debug, Clone)]
pub struct LocalContentStore {
    root: PathBuf,
}

impl LocalContentStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ContentStoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn put(&self, bytes: &[u8]) -> Result<StoredContent, ContentStoreError> {
        let sha256 = hex_digest(bytes);
        let path = self.path_for(&sha256)?;
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let temp = path.with_extension(format!("tmp-{}", std::process::id()));
            std::fs::write(&temp, bytes)?;
            match std::fs::rename(&temp, &path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let _ = std::fs::remove_file(temp);
                }
                Err(error) => {
                    let _ = std::fs::remove_file(temp);
                    return Err(error.into());
                }
            }
        }
        Ok(StoredContent {
            sha256: sha256.clone(),
            size_bytes: bytes.len() as u64,
            storage_ref: format!("local://{sha256}"),
        })
    }

    /// Store the bytes of a local file under their content address.
    pub fn put_file(
        &self,
        source: impl AsRef<std::path::Path>,
    ) -> Result<StoredContent, ContentStoreError> {
        let incoming = self.root.join(".incoming");
        std::fs::create_dir_all(&incoming)?;
        let temporary = incoming.join(format!("{}.part", std::process::id()));
        let result = (|| {
            let mut input = File::open(source)?;
            let mut output = File::create(&temporary)?;
            let mut digest = Sha256::new();
            let mut size = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = input.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                output.write_all(&buffer[..read])?;
                digest.update(&buffer[..read]);
                size = size
                    .checked_add(read as u64)
                    .ok_or(ContentStoreError::HashMismatch)?;
            }
            output.sync_all()?;
            let sha256 = format!("{:x}", digest.finalize());
            let path = self.path_for(&sha256)?;
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::rename(&temporary, &path)?;
            }
            Ok(StoredContent {
                sha256: sha256.clone(),
                size_bytes: size,
                storage_ref: format!("local://{sha256}"),
            })
        })();
        let _ = std::fs::remove_file(&temporary);
        result
    }

    pub fn get(&self, sha256: &str) -> Result<Option<Vec<u8>>, ContentStoreError> {
        let path = self.path_for(sha256)?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(path)?;
        if hex_digest(&bytes) != sha256 {
            return Err(ContentStoreError::HashMismatch);
        }
        Ok(Some(bytes))
    }

    /// Resolve an enterprise storage reference without exposing filesystem
    /// paths to callers.
    pub fn get_ref(&self, storage_ref: &str) -> Result<Option<Vec<u8>>, ContentStoreError> {
        let digest = storage_ref
            .strip_prefix("local://")
            .ok_or(ContentStoreError::InvalidHash)?;
        self.get(digest)
    }

    fn path_for(&self, sha256: &str) -> Result<PathBuf, ContentStoreError> {
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ContentStoreError::InvalidHash);
        }
        Ok(self.root.join(&sha256[..2]).join(sha256))
    }
}

impl ArtifactObjectStore for LocalContentStore {
    fn put_bytes(&self, bytes: &[u8]) -> Result<StoredContent, ContentStoreError> {
        self.put(bytes)
    }

    fn put_file(&self, source: &std::path::Path) -> Result<StoredContent, ContentStoreError> {
        LocalContentStore::put_file(self, source)
    }

    fn get_ref(&self, storage_ref: &str) -> Result<Option<Vec<u8>>, ContentStoreError> {
        LocalContentStore::get_ref(self, storage_ref)
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactObjectStore, ContentStoreError, LocalContentStore, S3CompatibleObjectStore,
    };
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn store() -> LocalContentStore {
        LocalContentStore::new(std::env::temp_dir().join(format!(
                "agentd-content-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            )))
        .expect("store")
    }

    #[test]
    fn put_and_get_are_content_addressed() {
        let store = store();
        let content = store.put(b"artifact bytes").expect("put");
        assert_eq!(content.size_bytes, 14);
        assert_eq!(
            store.get(&content.sha256).expect("get"),
            Some(b"artifact bytes".to_vec())
        );
        let _ = std::fs::remove_dir_all(store.root);
    }

    #[tokio::test]
    async fn s3_compatible_adapter_puts_gets_and_verifies_content() {
        let server = MockServer::start().await;
        let bytes = b"production-object";
        let digest = super::hex_digest(bytes);
        Mock::given(method("PUT"))
            .and(path(format!("/{digest}")))
            .and(header("authorization", "Bearer secret"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/{digest}")))
            .and(header("authorization", "Bearer secret"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .expect(1)
            .mount(&server)
            .await;

        let endpoint = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            let store =
                S3CompatibleObjectStore::new(endpoint, Some("secret".to_owned())).expect("adapter");
            let stored = store.put_bytes(bytes).expect("put");
            assert_eq!(stored.storage_ref, format!("s3://{digest}"));
            assert_eq!(
                store.get_ref(&stored.storage_ref).expect("get"),
                Some(bytes.to_vec())
            );
        })
        .await;
        result.expect("blocking object store task");
    }

    #[tokio::test]
    async fn s3_compatible_adapter_rejects_corrupt_response() {
        let server = MockServer::start().await;
        let digest = super::hex_digest(b"expected");
        Mock::given(method("GET"))
            .and(path(format!("/{digest}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"corrupt"))
            .mount(&server)
            .await;
        let endpoint = server.uri();
        tokio::task::spawn_blocking(move || {
            let store = S3CompatibleObjectStore::new(endpoint, None).expect("adapter");
            assert!(matches!(
                store.get_ref(&format!("s3://{digest}")),
                Err(ContentStoreError::HashMismatch)
            ));
        })
        .await
        .expect("blocking object store task");
    }

    #[test]
    fn corrupted_object_is_rejected() {
        let store = store();
        let content = store.put(b"original").expect("put");
        let path = store.path_for(&content.sha256).expect("path");
        std::fs::write(path, b"tampered").expect("tamper");
        assert!(matches!(
            store.get(&content.sha256),
            Err(ContentStoreError::HashMismatch)
        ));
        let _ = std::fs::remove_dir_all(store.root);
    }

    #[test]
    fn put_file_streams_into_the_same_content_addressed_layout() {
        let store = store();
        let source = std::env::temp_dir().join(format!(
            "agentd-content-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(&source, b"file-backed artifact").expect("source");
        let content = store.put_file(&source).expect("put file");
        assert_eq!(
            store.get(&content.sha256).expect("get"),
            Some(b"file-backed artifact".to_vec())
        );
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_dir_all(store.root);
    }

    #[test]
    fn storage_reference_rejects_host_paths_and_unknown_schemes() {
        let store = store();
        assert!(matches!(
            store.get_ref("/tmp/object"),
            Err(ContentStoreError::InvalidHash)
        ));
        assert!(matches!(
            store.get_ref("s3://bucket/object"),
            Err(ContentStoreError::InvalidHash)
        ));
        let _ = std::fs::remove_dir_all(store.root);
    }
}
