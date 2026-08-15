//! Download a release tarball and verify its SHA-256 checksum.
//!
//! The artifact is downloaded into a [`tempfile::NamedTempFile`] and the
//! digest verified against the published checksum *before* anything touches
//! the running binary. The caller controls when the temp file is persisted or
//! discarded.

use sha2::Digest;

use crate::error::SyscityError;
use crate::Result;

/// Download `url` into a temp file and verify its SHA-256 hex digest.
///
/// Returns the verified temp file (kept alive for the caller's use). An empty
/// `expected_sha256` skips verification (never used for real releases, but
/// handy for local end-to-end tests).
pub async fn download_and_verify(
    client: &reqwest::Client,
    url: &str,
    expected_sha256: &str,
) -> Result<tempfile::NamedTempFile> {
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(SyscityError::ExternalService {
            source: format!("Download failed: {} returned {}", url, resp.status()),
            cause: None,
        });
    }

    let bytes = resp.bytes().await?;

    if !expected_sha256.is_empty() {
        let actual = hex::encode(sha2::Sha256::digest(&bytes));
        if actual != expected_sha256 {
            return Err(SyscityError::Validation(format!(
                "SHA-256 mismatch for {url}: expected {expected_sha256}, got {actual}"
            )));
        }
    }

    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(&bytes)?;
    file.flush()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn download_verifies_matching_checksum() {
        let server = MockServer::start().await;
        let payload = b"hello update".to_vec();
        let digest = hex::encode(sha2::Sha256::digest(&payload));
        Mock::given(method("GET"))
            .and(path("/pkg.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let file = download_and_verify(&client, &format!("{}/pkg.tar.gz", server.uri()), &digest)
            .await
            .unwrap();

        let mut got = Vec::new();
        file.reopen()
            .unwrap()
            .take(1_000_000)
            .read_to_end(&mut got)
            .unwrap();
        assert_eq!(got, b"hello update");
    }

    #[tokio::test]
    async fn download_rejects_checksum_mismatch() {
        let server = MockServer::start().await;
        let payload = b"hello update".to_vec();
        Mock::given(method("GET"))
            .and(path("/pkg.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result =
            download_and_verify(&client, &format!("{}/pkg.tar.gz", server.uri()), "deadbeef").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn download_skips_verify_when_empty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pkg.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"data"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let file = download_and_verify(&client, &format!("{}/pkg.tar.gz", server.uri()), "")
            .await
            .unwrap();
        assert!(file.path().exists());
    }
}
