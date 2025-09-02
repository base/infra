use anyhow::Result;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{self as s3, operation::head_object::HeadObjectError};
use std::path::Path;
use tracing::{error, info};

#[derive(Debug, Clone)]
pub struct S3Manager {
    client: s3::Client,
    bucket: String,
    key_prefix: String,
}

impl S3Manager {
    pub async fn new(bucket: String, region: String, key_prefix: String) -> Result<Self> {
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region))
            .load()
            .await;

        let client = s3::Client::new(&config);

        Ok(Self {
            client,
            bucket,
            key_prefix,
        })
    }

    #[cfg(test)]
    pub async fn new_with_endpoint(
        bucket: String,
        key_prefix: String,
        endpoint_url: String,
    ) -> Result<Self> {
        let config = aws_sdk_s3::config::Builder::default()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(s3::config::Credentials::new(
                "fake", "fake", None, None, "test",
            ))
            .endpoint_url(endpoint_url)
            .build();

        let client = s3::Client::from_conf(config);

        client.create_bucket().bucket(bucket.clone()).send().await?;
        info!("Created bucket {}", bucket);

        Ok(Self {
            client,
            bucket,
            key_prefix,
        })
    }

    pub async fn upload_file(&self, local_path: &str, key_suffix: &str) -> Result<String> {
        let key = format!("{}{}", self.key_prefix, key_suffix);

        info!(
            message = "Uploading file to S3",
            local_path = %local_path,
            bucket = %self.bucket,
            key = %key
        );

        let file_size = std::fs::metadata(local_path)?.len();
        let body = s3::primitives::ByteStream::from_path(Path::new(local_path)).await?;

        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(body)
            .content_type("application/octet-stream")
            .metadata("file_size", file_size.to_string());

        match request.send().await {
            Ok(_) => {
                info!(
                    message = "Successfully uploaded file to S3",
                    bucket = %self.bucket,
                    key = %key,
                    file_size = file_size
                );
                Ok(key)
            }
            Err(e) => {
                error!(
                    message = "Failed to upload file to S3",
                    bucket = %self.bucket,
                    key = %key,
                    error = %e
                );
                Err(anyhow::anyhow!("S3 upload failed: {}", e))
            }
        }
    }

    pub async fn delete_file(&self, key: &str) -> Result<()> {
        info!(
            message = "Deleting file from S3",
            bucket = %self.bucket,
            key = %key
        );

        let request = self.client.delete_object().bucket(&self.bucket).key(key);

        match request.send().await {
            Ok(_) => {
                info!(
                    message = "Successfully deleted file from S3",
                    bucket = %self.bucket,
                    key = %key
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    message = "Failed to delete file from S3",
                    bucket = %self.bucket,
                    key = %key,
                    error = %e
                );
                Err(anyhow::anyhow!("S3 delete failed: {}", e))
            }
        }
    }

    pub async fn file_exists(&self, key: &str) -> Result<bool> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => match e.into_service_error() {
                HeadObjectError::NotFound(_) => Ok(false),
                err => Err(anyhow::anyhow!("S3 head_object failed: {:?}", err)),
            },
        }
    }

    pub async fn list_files(&self) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut continuation_token = None;

        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&self.key_prefix);

            if let Some(token) = &continuation_token {
                request = request.continuation_token(token);
            }

            let response = request.send().await?;

            for object in response.contents() {
                if let Some(key) = object.key() {
                    keys.push(key.to_string());
                }
            }

            if response.is_truncated() == Some(true) {
                continuation_token = response.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(keys)
    }

    pub fn generate_archive_key(&self, start_block: u64, end_block: u64) -> String {
        format!("flashblocks_archive_{}_{}.parquet", start_block, end_block)
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn key_prefix(&self) -> &str {
        &self.key_prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_archive_key() {
        let s3_manager = S3Manager {
            client: s3::Client::new(&aws_config::SdkConfig::builder().build()),
            bucket: "test-bucket".to_string(),
            key_prefix: "flashblocks/".to_string(),
        };

        let key = s3_manager.generate_archive_key(1000, 2000);
        assert_eq!(key, "flashblocks_archive_1000_2000.parquet");
    }
}
