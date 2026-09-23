use anyhow::{Context, Result, ensure};
use aws_sdk_s3::{
    Client,
    config::{
        BehaviorVersion, Credentials, Region, RequestChecksumCalculation, retry::RetryConfig,
        timeout::TimeoutConfig,
    },
    error::{ProvideErrorMetadata, SdkError},
    primitives::{ByteStream, Length},
    types::{CompletedMultipartUpload, CompletedPart},
};
use std::{path::Path, time::Duration};

const PART_SIZE: u64 = 8 * 1024 * 1024;
const MAX_PARTS: u64 = 10_000;

#[derive(Clone, PartialEq, Eq)]
pub enum StorageConfig {
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        root: String,
        public_base_url: String,
        access_key_id: String,
        secret_access_key: String,
    },
}
impl StorageConfig {
    pub fn root(&self) -> &str {
        match self {
            Self::S3 { root, .. } => root,
        }
    }
    pub fn public_base(&self) -> &str {
        match self {
            Self::S3 {
                public_base_url, ..
            } => public_base_url,
        }
    }
    pub fn identity(&self) -> String {
        match self {
            Self::S3 {
                endpoint,
                region,
                bucket,
                root,
                public_base_url,
                ..
            } => format!("s3:{endpoint}:{region}:{bucket}:{root}:{public_base_url}"),
        }
    }
}

#[derive(Clone)]
pub struct Storage {
    client: Client,
    bucket: String,
    config: StorageConfig,
}
impl Storage {
    pub fn new(config: StorageConfig) -> Result<Self> {
        validate_url(config.public_base())?;
        validate_key(config.root().trim_matches('/'), true)?;
        let StorageConfig::S3 {
            endpoint,
            region,
            bucket,
            access_key_id,
            secret_access_key,
            ..
        } = &config;
        validate_url(endpoint)?;
        ensure!(
            !bucket.is_empty()
                && !region.is_empty()
                && !access_key_id.is_empty()
                && !secret_access_key.is_empty(),
            "Complete the S3 credentials"
        );
        let client = Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .endpoint_url(endpoint)
                .region(Region::new(region.clone()))
                .credentials_provider(Credentials::new(
                    access_key_id,
                    secret_access_key,
                    None,
                    None,
                    "Mazit",
                ))
                .force_path_style(true)
                .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
                .retry_config(RetryConfig::standard().with_max_attempts(3))
                .timeout_config(
                    TimeoutConfig::builder()
                        .connect_timeout(Duration::from_secs(10))
                        .operation_attempt_timeout(Duration::from_secs(120))
                        .operation_timeout(Duration::from_secs(360))
                        .build(),
                )
                .build(),
        );
        Ok(Self {
            client,
            bucket: bucket.clone(),
            config,
        })
    }
    pub fn url(&self, key: &str) -> Result<String> {
        validate_key(key, false)?;
        let mut url = url::Url::parse(self.config.public_base())?;
        let mut parts = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid public base URL"))?;
        parts.pop_if_empty();
        for part in self
            .config
            .root()
            .trim_matches('/')
            .split('/')
            .chain(key.split('/'))
            .filter(|part| !part.is_empty())
        {
            parts.push(part);
        }
        drop(parts);
        Ok(url.into())
    }
    fn object_key(&self, key: &str) -> Result<String> {
        validate_key(key, false)?;
        let root = self.config.root().trim_matches('/');
        Ok(if root.is_empty() {
            key.to_string()
        } else {
            format!("{root}/{key}")
        })
    }
    pub async fn put_file(&self, key: &str, file: &Path) -> Result<()> {
        let key = self.object_key(key)?;
        let size = tokio::fs::metadata(file).await?.len();
        if size <= PART_SIZE {
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .body(ByteStream::from_path(file).await?)
                .content_type("audio/mp4")
                .cache_control("public, max-age=3600")
                .send()
                .await
                .map_err(|error| storage_error("upload", error))?;
            return Ok(());
        }
        let part_size = PART_SIZE.max(size.div_ceil(MAX_PARTS));
        ensure!(
            part_size <= 5 * 1024 * 1024 * 1024,
            "Audio file exceeds the S3 multipart upload size limit"
        );
        let upload = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(&key)
            .content_type("audio/mp4")
            .cache_control("public, max-age=3600")
            .send()
            .await
            .map_err(|error| storage_error("start multipart upload", error))?;
        let upload_id = upload
            .upload_id()
            .context("S3 did not return a multipart upload ID")?;
        let result = async {
            let mut parts = Vec::new();
            for index in 0..size.div_ceil(part_size) {
                let offset = index * part_size;
                let body = ByteStream::read_from()
                    .path(file)
                    .offset(offset)
                    .length(Length::Exact((size - offset).min(part_size)))
                    .build()
                    .await?;
                let part_number = (index + 1) as i32;
                let part = self
                    .client
                    .upload_part()
                    .bucket(&self.bucket)
                    .key(&key)
                    .upload_id(upload_id)
                    .part_number(part_number)
                    .body(body)
                    .send()
                    .await
                    .map_err(|error| storage_error("upload part", error))?;
                parts.push(
                    CompletedPart::builder()
                        .part_number(part_number)
                        .e_tag(
                            part.e_tag()
                                .context("S3 did not return an uploaded part ETag")?,
                        )
                        .build(),
                );
            }
            self.client
                .complete_multipart_upload()
                .bucket(&self.bucket)
                .key(&key)
                .upload_id(upload_id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(parts))
                        .build(),
                )
                .send()
                .await
                .map_err(|error| storage_error("complete multipart upload", error))?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = self
                .client
                .abort_multipart_upload()
                .bucket(&self.bucket)
                .key(&key)
                .upload_id(upload_id)
                .send()
                .await;
        }
        result
    }
    pub async fn put_text(&self, key: &str, text: String, mime: &str) -> Result<()> {
        self.put_bytes(key, text.into_bytes(), mime).await
    }
    pub async fn put_bytes(&self, key: &str, bytes: Vec<u8>, mime: &str) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.object_key(key)?)
            .body(ByteStream::from(bytes))
            .content_type(mime)
            .cache_control("no-cache")
            .send()
            .await
            .map_err(|error| storage_error("upload", error))?;
        Ok(())
    }
    pub async fn delete(&self, key: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.object_key(key)?)
            .send()
            .await
            .map_err(|error| storage_error("delete", error))?;
        Ok(())
    }
    pub async fn verify(&self) -> Result<()> {
        let key = format!("mazit-check-{}.txt", uuid::Uuid::new_v4());
        let text = uuid::Uuid::new_v4().to_string();
        let result = async {
            self.put_text(&key, text.clone(), "text/plain").await?;
            let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
            let response = client.get(self.url(&key)?).send().await.map_err(|e| e.without_url())?;
            ensure!(response.status().is_success() && response.text().await? == text, "The public URL must serve uploaded file contents directly. Sharing pages and expiring URLs cannot host a podcast feed.");
            Ok(())
        }.await;
        let deleted = self.delete(&key).await;
        result?;
        deleted
    }
}

fn storage_error<E: ProvideErrorMetadata>(operation: &str, error: SdkError<E>) -> anyhow::Error {
    // Provider messages and raw SDK error chains can contain request credentials or URLs.
    let reason = match &error {
        SdkError::ServiceError(context) => context.err().code().unwrap_or("provider error"),
        SdkError::TimeoutError(_) => "request timed out",
        SdkError::DispatchFailure(_) => "connection failed",
        SdkError::ConstructionFailure(_) => "invalid request configuration",
        _ => "invalid provider response",
    };
    anyhow::anyhow!("S3 {operation} failed: {reason}")
}

fn validate_url(text: &str) -> Result<()> {
    let url = url::Url::parse(text)?;
    ensure!(
        url.scheme() == "https"
            || (url.scheme() == "http"
                && ["127.0.0.1", "localhost", "[::1]"].contains(&url.host_str().unwrap_or(""))),
        "Storage URLs require HTTPS (HTTP is allowed on loopback for development)"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Storage URLs must not contain credentials, queries, or fragments"
    );
    Ok(())
}
fn validate_key(key: &str, empty: bool) -> Result<()> {
    ensure!(
        (empty && key.is_empty())
            || (!key.is_empty()
                && !key.contains('\\')
                && key
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..")),
        "Invalid storage path"
    );
    Ok(())
}
