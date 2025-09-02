use anyhow::Result;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use std::path::Path;
use tracing::{error, info};

#[derive(Debug)]
pub struct S3Manager {
    client: S3Client,
    bucket: String,
    key_prefix: String,
}

impl S3Manager {
    pub async fn new(bucket: String, region: String, key_prefix: String) -> Result<Self> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region))
            .load()
            .await;

        let client = S3Client::new(&config);

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
        let body = ByteStream::from_path(Path::new(local_path)).await?;

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
            Err(e) => {
                if e.to_string().contains("NotFound") || e.to_string().contains("404") {
                    Ok(false)
                } else {
                    Err(anyhow::anyhow!("S3 head_object failed: {}", e))
                }
            }
        }
    }

    pub async fn list_files(&self, prefix: Option<&str>) -> Result<Vec<String>> {
        let list_prefix = match prefix {
            Some(p) => format!("{}{}", self.key_prefix, p),
            None => self.key_prefix.clone(),
        };

        let mut keys = Vec::new();
        let mut continuation_token = None;

        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&list_prefix);

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
            client: S3Client::new(&aws_config::SdkConfig::builder().build()),
            bucket: "test-bucket".to_string(),
            key_prefix: "flashblocks/".to_string(),
        };

        let key = s3_manager.generate_archive_key(1000, 2000);
        assert_eq!(key, "flashblocks_archive_1000_2000.parquet");
    }
}
