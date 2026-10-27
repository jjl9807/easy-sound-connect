//! Product artwork catalog and disk cache; never participates in device control.

use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, RequestBuilder, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::Semaphore;

const API_URL: &str = "https://v1.api.data-gateway.seeds.services/graphql";
const USER_AGENT: &str = "Phone (Android 16; Pixel 9 Pro) com.sony.songpal.mdr/13.3.0";
const API_KEY: &str = "37ksfjEuDRYXYtgus4rEDw6QC2NqhTfr";
const QUERY: &str = "query GetAllCloudModelInfos { HPC { getAllCloudModelInfos { wp_check_left_image_url wp_check_right_image_url model_id model_number is_official_model_name model_name model_color_id sca_image_image_url sca_anime_image_url sca_source_color model_category big_header_bg_gradient_color_1 big_header_bg_gradient_color_2 big_header_theme_label tutorial_home_page_numbers tutorial_page_filter_displayed_on_oobe wp_left_image_url wp_right_image_url } } }";
const CACHE_VERSION: u32 = 1;
const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
const MAX_MODELS: usize = 4096;
const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;
const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CACHE_FILES: usize = 128;
const ASSET_MAGIC: &[u8; 8] = b"EZSCIMG1";
const ASSET_HEADER_BYTES: usize = ASSET_MAGIC.len() + 32;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ModelKey {
    name: String,
    color: u8,
}

impl ModelKey {
    pub(super) fn new(name: impl AsRef<str>, color: u8) -> Self {
        Self {
            name: name.as_ref().trim().to_ascii_uppercase(),
            color,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AssetUrls {
    pub(super) image: String,
    pub(super) animation: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Catalog {
    entries: HashMap<ModelKey, AssetUrls>,
}

impl Catalog {
    pub(super) fn lookup(&self, key: &ModelKey) -> Option<&AssetUrls> {
        self.entries.get(key)
    }

    fn from_models(models: &[ModelInfo], allow_http: bool) -> Result<Self> {
        ensure!(models.len() <= MAX_MODELS, "Product catalog is too large");
        let mut entries = HashMap::new();
        let mut ambiguous = HashSet::new();
        for model in models {
            let (Some(name), Some(color), Some(image)) = (
                model.model_name.as_deref(),
                model.model_color_id.as_deref(),
                model.sca_image_image_url.as_deref(),
            ) else {
                continue;
            };
            let Some(color) = parse_color(color) else {
                continue;
            };
            if name.trim().is_empty()
                || name.len() > 256
                || resource_url(image, allow_http).is_err()
            {
                continue;
            }
            let key = ModelKey::new(name, color);
            let urls = AssetUrls {
                image: image.to_owned(),
                animation: model
                    .sca_anime_image_url
                    .as_ref()
                    .filter(|url| resource_url(url, allow_http).is_ok())
                    .cloned(),
            };
            if entries.get(&key).is_some_and(|previous| previous != &urls) {
                ambiguous.insert(key.clone());
            }
            entries.insert(key, urls);
        }
        // Conflicting records must not silently display an arbitrary colorway.
        for key in ambiguous {
            entries.remove(&key);
        }
        ensure!(!entries.is_empty(), "Product catalog has no usable artwork");
        Ok(Self { entries })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelInfo {
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    model_color_id: Option<String>,
    #[serde(default)]
    sca_image_image_url: Option<String>,
    #[serde(default)]
    sca_anime_image_url: Option<String>,
}

#[derive(Deserialize)]
struct Response {
    data: Option<ResponseData>,
    #[serde(default)]
    errors: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct ResponseData {
    #[serde(rename = "HPC")]
    hpc: ResponseModels,
}

#[derive(Deserialize)]
struct ResponseModels {
    #[serde(rename = "getAllCloudModelInfos")]
    models: Vec<ModelInfo>,
}

#[derive(Serialize)]
struct CatalogRequest {
    query: &'static str,
}

#[derive(Deserialize, Serialize)]
struct CachedCatalog {
    version: u32,
    models: Vec<ModelInfo>,
}

#[derive(Clone)]
pub(super) struct AssetStore {
    client: Client,
    directory: PathBuf,
    endpoint: Url,
    allow_http: bool,
    requests: Arc<Semaphore>,
}

impl AssetStore {
    pub(super) fn new() -> Result<Self> {
        Self::at(crate::config::directory()?.join("cache/products"))
    }

    pub(super) fn at(directory: PathBuf) -> Result<Self> {
        Self::build(directory, Url::parse(API_URL)?, false)
    }

    fn build(directory: PathBuf, endpoint: Url, allow_http: bool) -> Result<Self> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(25))
            // The catalog credential must never follow a redirect to another origin.
            .redirect(reqwest::redirect::Policy::none())
            .https_only(!allow_http)
            .build()?;
        Ok(Self {
            client,
            directory,
            endpoint,
            allow_http,
            requests: Arc::new(Semaphore::new(3)),
        })
    }

    pub(super) async fn catalog(&self) -> Result<Catalog> {
        let path = self.directory.join("catalog.json");
        let read_path = path.clone();
        let allow_http = self.allow_http;
        if let Ok(Ok(catalog)) = tokio::task::spawn_blocking(move || -> Result<Catalog> {
            let bytes = read_bounded(&read_path, MAX_CATALOG_BYTES)?;
            let cache: CachedCatalog = serde_json::from_slice(&bytes)?;
            ensure!(
                cache.version == CACHE_VERSION,
                "Unsupported product catalog cache"
            );
            Catalog::from_models(&cache.models, allow_http)
        })
        .await
        {
            return Ok(catalog);
        }

        let _permit = self.requests.acquire().await?;
        let mut api_key = reqwest::header::HeaderValue::from_static(API_KEY);
        api_key.set_sensitive(true);
        let bytes = fetch(
            self.client
                .post(self.endpoint.clone())
                .header("x-api-key", api_key)
                .json(&CatalogRequest { query: QUERY }),
            MAX_CATALOG_BYTES,
        )
        .await?;
        let response: Response =
            serde_json::from_slice(&bytes).context("Invalid product catalog")?;
        ensure!(
            response.errors.is_empty(),
            "Product catalog service returned GraphQL errors"
        );
        let models = response
            .data
            .context("Product catalog has no data")?
            .hpc
            .models;
        let catalog = Catalog::from_models(&models, self.allow_http)?;
        // A read-only or full cache directory must not discard a usable response.
        let _ = tokio::task::spawn_blocking(move || -> Result<()> {
            let bytes = serde_json::to_vec(&CachedCatalog {
                version: CACHE_VERSION,
                models,
            })?;
            atomic_write(&path, &bytes)
        })
        .await;
        Ok(catalog)
    }

    pub(super) async fn load_asset(&self, url: &str) -> Result<Vec<u8>> {
        let url = resource_url(url, self.allow_http)?;
        let path = self.asset_path(url.as_str());
        let read_path = path.clone();
        if let Ok(Ok(bytes)) = tokio::task::spawn_blocking(move || read_asset(&read_path)).await {
            return Ok(bytes);
        }
        let _permit = self.requests.acquire().await?;
        let bytes = fetch(self.client.get(url), MAX_ASSET_BYTES).await?;
        ensure!(!bytes.is_empty(), "Product image is empty");
        let cache_bytes = bytes.clone();
        let directory = self.directory.join("images");
        let _ = tokio::task::spawn_blocking(move || -> Result<()> {
            let mut encoded = Vec::with_capacity(ASSET_HEADER_BYTES + cache_bytes.len());
            encoded.extend_from_slice(ASSET_MAGIC);
            encoded.extend_from_slice(&Sha256::digest(&cache_bytes));
            encoded.extend_from_slice(&cache_bytes);
            atomic_write(&path, &encoded)?;
            prune_images(&directory, &path);
            Ok(())
        })
        .await;
        Ok(bytes)
    }

    pub(super) async fn invalidate_asset(&self, url: &str) -> Result<()> {
        let url = resource_url(url, self.allow_http)?;
        let path = self.asset_path(url.as_str());
        tokio::task::spawn_blocking(move || match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        })
        .await?
    }

    fn asset_path(&self, url: &str) -> PathBuf {
        self.directory
            .join("images")
            .join(format!("{:x}.asset", Sha256::digest(url.as_bytes())))
    }
}

fn parse_color(value: &str) -> Option<u8> {
    let value = value.trim();
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))?;
    if hex.is_empty() || hex.len() > 2 {
        return None;
    }
    u8::from_str_radix(hex, 16).ok()
}

fn resource_url(value: &str, allow_http: bool) -> Result<Url> {
    ensure!(value.len() <= 4096, "Product image URL is too long");
    let url = Url::parse(value).context("Invalid product image URL")?;
    ensure!(
        url.scheme() == "https" || (allow_http && url.scheme() == "http"),
        "Product image requires HTTPS"
    );
    ensure!(
        url.host_str().is_some() && url.username().is_empty() && url.password().is_none(),
        "Invalid product image origin"
    );
    ensure!(
        url.fragment().is_none(),
        "Product image URL must not contain a fragment"
    );
    Ok(url)
}

async fn fetch(request: RequestBuilder, max_bytes: usize) -> Result<Vec<u8>> {
    for attempt in 0..2 {
        let result = request
            .try_clone()
            .context("Product request cannot be retried")?
            .send()
            .await;
        let mut response = match result {
            Ok(response)
                if attempt == 0
                    && (response.status().is_server_error()
                        || response.status().as_u16() == 429) =>
            {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            Err(_) if attempt == 0 => {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            result => result.context("Unable to download product artwork")?,
        };
        ensure!(
            response.status().is_success(),
            "Product artwork service returned HTTP {}",
            response.status().as_u16()
        );
        if let Some(length) = response.content_length() {
            ensure!(
                length <= max_bytes as u64,
                "Product artwork response is too large"
            );
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= max_bytes,
                "Product artwork response is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        return Ok(bytes);
    }
    bail!("Unable to download product artwork")
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= limit as u64,
        "Product cache file is too large"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "Product cache file is too large");
    Ok(bytes)
}

fn read_asset(path: &Path) -> Result<Vec<u8>> {
    let bytes = read_bounded(path, MAX_ASSET_BYTES + ASSET_HEADER_BYTES)?;
    ensure!(
        bytes.len() > ASSET_HEADER_BYTES && bytes.starts_with(ASSET_MAGIC),
        "Invalid product image cache"
    );
    let image = &bytes[ASSET_HEADER_BYTES..];
    ensure!(
        Sha256::digest(image).as_slice() == &bytes[ASSET_MAGIC.len()..ASSET_HEADER_BYTES],
        "Corrupt product image cache"
    );
    Ok(image.to_vec())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("Product cache has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

fn prune_images(directory: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "asset")
                || !entry.file_type().ok()?.is_file()
            {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((
                entry.path(),
                metadata.len(),
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            ))
        })
        .collect();
    let mut size: u64 = files.iter().map(|(_, size, _)| size).sum();
    let mut count = files.len();
    files.sort_by_key(|(_, _, modified)| *modified);
    for (path, length, _) in files {
        if count <= MAX_CACHE_FILES && size <= MAX_CACHE_BYTES {
            break;
        }
        if path != keep && fs::remove_file(path).is_ok() {
            count -= 1;
            size = size.saturating_sub(length);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn model(name: &str, color: &str, image: &str) -> ModelInfo {
        ModelInfo {
            model_name: Some(name.into()),
            model_color_id: Some(color.into()),
            sca_image_image_url: Some(image.into()),
            sca_anime_image_url: None,
        }
    }

    fn response(models: &[ModelInfo]) -> Vec<u8> {
        serde_json::to_vec(
            &serde_json::json!({ "data": { "HPC": { "getAllCloudModelInfos": models } } }),
        )
        .unwrap()
    }

    async fn server(replies: Vec<(u16, Vec<u8>)>) -> (Url, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/graphql",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in replies {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(3), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let length = socket.read(&mut buf).await.unwrap();
                    assert_ne!(length, 0);
                    request.extend_from_slice(&buf[..length]);
                    if let Some(header_end) =
                        request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let headers =
                            String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
                        let body_length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .map(|length| length.parse().unwrap())
                            .unwrap_or(0);
                        if request.len() >= header_end + 4 + body_length {
                            break;
                        }
                    }
                }
                requests.push(request);
                let headers = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(headers.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
            requests
        });
        (endpoint, task)
    }

    #[test]
    fn catalog_requires_matching_color_and_rejects_ambiguous_assets() {
        let catalog = Catalog::from_models(
            &[
                model("WF-Test", "0x00", "https://example.com/black.png"),
                model("WF-Test", "0X03", "https://example.com/silver.png"),
                model("WH-Test", "0x00", "https://example.com/a.png"),
                model("WH-Test", "0x00", "https://example.com/b.png"),
                model("Bad color", "0x100", "https://example.com/unused.png"),
                model("Unsafe URL", "0x00", "file:///private/image.png"),
            ],
            false,
        )
        .unwrap();
        assert_eq!(
            catalog
                .lookup(&ModelKey::new(" wf-test ", 3))
                .unwrap()
                .image,
            "https://example.com/silver.png"
        );
        assert!(catalog.lookup(&ModelKey::new("WF-Test", 1)).is_none());
        assert!(catalog.lookup(&ModelKey::new("WH-Test", 0)).is_none());
        assert!(catalog.lookup(&ModelKey::new("Bad color", 0)).is_none());
        assert!(catalog.lookup(&ModelKey::new("Unsafe URL", 0)).is_none());
    }

    #[tokio::test]
    async fn serializes_catalog_request_and_reuses_disk_cache_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let image = b"test image bytes".to_vec();
        let (endpoint, requests) = server(vec![
            (
                200,
                response(&[model("WF-Test", "0x03", "https://example.com/product.png")]),
            ),
            (200, image.clone()),
        ])
        .await;
        let asset_url = endpoint.join("image.png").unwrap().to_string();
        let store = AssetStore::build(directory.path().into(), endpoint.clone(), true).unwrap();
        let catalog = store.catalog().await.unwrap();
        assert!(catalog.lookup(&ModelKey::new("WF-Test", 3)).is_some());
        assert_eq!(store.load_asset(&asset_url).await.unwrap(), image);
        let requests = requests.await.unwrap();
        let first = String::from_utf8(requests[0].clone()).unwrap();
        let (headers, body) = first.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("POST /graphql HTTP/1.1"));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        assert!(headers.contains(USER_AGENT));
        assert!(headers.contains(API_KEY));
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["query"], QUERY);
        assert!(
            !String::from_utf8_lossy(&requests[1])
                .to_ascii_lowercase()
                .contains("x-api-key")
        );
        // The server is gone: both operations must succeed from on-disk data.
        let restarted = AssetStore::build(directory.path().into(), endpoint, true).unwrap();
        assert!(
            restarted
                .catalog()
                .await
                .unwrap()
                .lookup(&ModelKey::new("WF-Test", 3))
                .is_some()
        );
        assert_eq!(restarted.load_asset(&asset_url).await.unwrap(), image);
    }

    #[tokio::test]
    async fn repairs_corrupt_catalog_and_asset_cache_without_using_bad_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let image = b"replacement bytes".to_vec();
        let (endpoint, requests) = server(vec![
            (
                200,
                response(&[model("WF-Test", "0x00", "https://example.com/product.png")]),
            ),
            (200, image.clone()),
        ])
        .await;
        let store = AssetStore::build(directory.path().into(), endpoint.clone(), true).unwrap();
        fs::write(directory.path().join("catalog.json"), b"{corrupt").unwrap();
        let asset_url = endpoint.join("image.png").unwrap().to_string();
        let path = store.asset_path(&asset_url);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut broken = Vec::from(ASSET_MAGIC.as_slice());
        broken.extend([0; 32]);
        broken.extend(b"bad pixels");
        fs::write(&path, broken).unwrap();
        assert!(
            store
                .catalog()
                .await
                .unwrap()
                .lookup(&ModelKey::new("WF-Test", 0))
                .is_some()
        );
        assert_eq!(store.load_asset(&asset_url).await.unwrap(), image);
        requests.await.unwrap();
        assert_eq!(read_asset(&path).unwrap(), image);
        store.invalidate_asset(&asset_url).await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn failed_graphql_response_does_not_poison_catalog_cache() {
        let directory = tempfile::tempdir().unwrap();
        let (endpoint, requests) = server(vec![(
            200,
            br#"{"data":null,"errors":[{"message":"denied"}]}"#.to_vec(),
        )])
        .await;
        let store = AssetStore::build(directory.path().into(), endpoint, true).unwrap();
        assert!(store.catalog().await.is_err());
        requests.await.unwrap();
        assert!(!directory.path().join("catalog.json").exists());
    }

    #[tokio::test]
    async fn transient_server_failure_retries_once_and_oversized_body_is_rejected() {
        let (endpoint, requests) =
            server(vec![(503, Vec::new()), (200, b"recovered".to_vec())]).await;
        let client = Client::new();
        assert_eq!(fetch(client.get(endpoint), 16).await.unwrap(), b"recovered");
        assert_eq!(requests.await.unwrap().len(), 2);
        let (endpoint, requests) = server(vec![(200, vec![0; 20])]).await;
        assert!(fetch(client.get(endpoint), 10).await.is_err());
        requests.await.unwrap();
    }

    #[test]
    fn cache_filenames_do_not_include_remote_paths_and_urls_require_https() {
        let store =
            AssetStore::build(PathBuf::from("cache"), Url::parse(API_URL).unwrap(), false).unwrap();
        let path = store.asset_path("https://example.com/../../private.png?token=abc");
        assert_eq!(path.parent().unwrap(), Path::new("cache/images"));
        assert_eq!(path.file_name().unwrap().to_str().unwrap().len(), 70);
        for url in [
            "file:///tmp/image.png",
            "http://example.com/a.png",
            "https://user:password@example.com/a.png",
        ] {
            assert!(resource_url(url, false).is_err());
        }
        assert!(resource_url("https://example.com/a.png", false).is_ok());
    }
}
