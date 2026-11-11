use std::{io::Read, sync::Arc};

use image::{ImageReader, Limits};
use url::Url;

use crate::{
    DownloadError, FetchError, FetchRequest, HttpError, HttpRequest, HttpTransport, LocalStash,
    ModelBytes, ModelFetcher, ModelSource, OriginPolicy, ResponseKind,
};

const MAX_MODEL_BYTES: usize = 50 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 8192;
/// A 4096-pixel square. This holds a large cover image, and a phone photo of 12 megapixels.
const MAX_IMAGE_PIXELS: u64 = 4096 * 4096;
/// Eight bits for each channel at [`MAX_IMAGE_PIXELS`], with room for the work of a decoder.
const MAX_IMAGE_ALLOC: u64 = 72 * 1024 * 1024;

/// Decoded RGBA image with bounded dimensions and allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Four bytes per pixel in row-major RGBA order.
    pub rgba: Vec<u8>,
}

/// Async model and image downloader. Clones share one transport and one policy.
pub struct DownloadClient<T: HttpTransport> {
    transport: Arc<T>,
    policy: Arc<OriginPolicy>,
    stash: Option<Arc<LocalStash>>,
}

impl<T: HttpTransport> Clone for DownloadClient<T> {
    fn clone(&self) -> Self {
        Self {
            transport: Arc::clone(&self.transport),
            policy: Arc::clone(&self.policy),
            stash: self.stash.clone(),
        }
    }
}

impl<T: HttpTransport> DownloadClient<T> {
    /// Creates a downloader with an explicit model-origin allowlist.
    #[must_use]
    pub fn new(transport: T, policy: OriginPolicy) -> Self {
        Self {
            transport: Arc::new(transport),
            policy: Arc::new(policy),
            stash: None,
        }
    }

    /// Enables local-stash sources through the same fetch contract.
    #[must_use]
    pub fn with_stash(mut self, stash: LocalStash) -> Self {
        self.stash = Some(Arc::new(stash));
        self
    }

    /// Downloads one nonempty model with at most one transient retry.
    ///
    /// Only the upstream origin receives the bearer token. The other allowed origins are public.
    ///
    /// # Errors
    ///
    /// Returns origin, transport, status, timeout, size, local I/O, or empty-download errors.
    pub async fn download_model(&self, request: FetchRequest) -> Result<ModelBytes, DownloadError> {
        let ModelSource::Catalog { url } = &request.source else {
            return self.download_local(&request.source).await;
        };
        self.policy.validate(url).map_err(|_| {
            eprintln!("{}", refused_model_note(url));
            DownloadError::UnsafeOrigin
        })?;
        let bearer = request
            .bearer
            .as_ref()
            .filter(|_| self.policy.permits_credentials(url));
        for attempt in 0..2 {
            let result = self
                .transport
                .execute(HttpRequest {
                    headers: match bearer {
                        Some(token) => {
                            use secrecy::ExposeSecret;
                            vec![(
                                "Authorization".into(),
                                format!("Bearer {}", token.expose_secret()),
                            )]
                        }
                        None => Vec::new(),
                    },
                    ..HttpRequest::get(url.clone(), ResponseKind::Model)
                })
                .await;
            match result {
                Ok(response) if (200..300).contains(&response.status) => {
                    if response.body.is_empty() {
                        return Err(DownloadError::EmptyDownload);
                    }
                    return Ok(ModelBytes::from(response.body));
                }
                Ok(response) if response.status == 401 => {
                    return Err(DownloadError::Unauthorized);
                }
                Ok(response) if attempt == 0 && transient_status(response.status) => continue,
                Ok(response) => return Err(DownloadError::HttpStatus(response.status)),
                Err(error) if attempt == 0 && transient_error(&error) => continue,
                Err(error) => return Err(map_http(error)),
            }
        }
        Err(DownloadError::Transport)
    }

    /// Downloads model bytes with one refresh and one retry after HTTP 401 from upstream.
    ///
    /// A public origin receives no token, so this path does not read the session and a refresh
    /// cannot change its answer.
    ///
    /// # Errors
    ///
    /// Returns authentication, session, origin, transport, timeout, size, or local I/O errors.
    pub async fn download_model_with_tokens(
        &self,
        mut request: FetchRequest,
        tokens: &(impl crate::TokenProvider + ?Sized),
    ) -> Result<ModelBytes, DownloadError> {
        let upstream = matches!(
            &request.source,
            ModelSource::Catalog { url } if self.policy.permits_credentials(url)
        );
        if !upstream {
            return self.download_model(request).await;
        }
        if request.bearer.is_none() {
            request.bearer = tokens.access_token(false).await.map_err(map_api)?;
        }
        match self.download_model(request.clone()).await {
            Err(DownloadError::Unauthorized) => {
                let replacement = match &request.bearer {
                    Some(rejected) => tokens.refreshed_token(rejected).await,
                    None => tokens.access_token(true).await,
                };
                let fresh = replacement
                    .map_err(map_api)?
                    .ok_or(DownloadError::Unauthorized)?;
                request.bearer = Some(fresh);
                self.download_model(request).await
            }
            result => result,
        }
    }

    /// Downloads and decodes one image. HTTP 404 returns `Ok(None)` for fallback art.
    ///
    /// # Errors
    ///
    /// Returns origin, transport, status, size, or bounded image-decode errors.
    pub async fn download_image(&self, url: &Url) -> Result<Option<DecodedImage>, DownloadError> {
        self.policy
            .validate(url)
            .map_err(|_| DownloadError::UnsafeOrigin)?;
        let response = self
            .transport
            .execute(HttpRequest::get(url.clone(), ResponseKind::Image))
            .await
            .map_err(map_http)?;
        if response.status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&response.status) {
            return Err(DownloadError::HttpStatus(response.status));
        }
        // This reader reads only the header, so a refused image allocates no pixels.
        let (width, height) = bounded_image_reader(&response.body)?
            .into_dimensions()
            .map_err(|_| DownloadError::Image)?;
        if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
            return Err(DownloadError::Image);
        }
        let image = bounded_image_reader(&response.body)?
            .decode()
            .map_err(|_| DownloadError::Image)?
            .into_rgba8();
        let (width, height) = image.dimensions();
        Ok(Some(DecodedImage {
            width,
            height,
            rgba: image.into_raw(),
        }))
    }

    async fn download_local(&self, source: &ModelSource) -> Result<ModelBytes, DownloadError> {
        let ModelSource::Local { stash_name } = source else {
            return Err(DownloadError::UnsafeOrigin);
        };
        let stash = self.stash.as_ref().ok_or(DownloadError::LocalUnavailable)?;
        let source_path = std::path::Path::new("/").join(stash_name);
        let source_url =
            Url::from_file_path(source_path).map_err(|_| DownloadError::UnsafeOrigin)?;
        let path = stash.resolve(source_url.as_str())?;
        let file = std::fs::File::open(&path)?;
        let mut bytes = Vec::new();
        file.take(MAX_MODEL_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_MODEL_BYTES {
            return Err(DownloadError::ResponseTooLarge {
                max: MAX_MODEL_BYTES,
            });
        }
        if bytes.is_empty() {
            return Err(DownloadError::EmptyDownload);
        }
        stash.touch(&path)?;
        Ok(ModelBytes::from(bytes))
    }
}

impl<T: HttpTransport + Send + Sync> ModelFetcher for DownloadClient<T> {
    async fn fetch(&self, request: FetchRequest) -> Result<ModelBytes, FetchError> {
        self.download_model(request)
            .await
            .map_err(|error| match error {
                DownloadError::EmptyDownload => FetchError::EmptyDownload,
                DownloadError::Unauthorized => FetchError::Unauthorized,
                _ => FetchError::Failed("model download failed"),
            })
    }
}

/// The terminal note for a refused model address. It names only the host, because the rest of the
/// address can hold a signature. A refused redirect has its own note, from the transport.
fn refused_model_note(url: &Url) -> String {
    let host = url.host_str().unwrap_or("with no name");
    format!(
        "Ronnie refused a model on the host {host}. To allow this host, add its HTTPS origin to RONIE_MODEL_ORIGINS."
    )
}

fn bounded_image_reader(
    bytes: &[u8],
) -> Result<ImageReader<std::io::Cursor<&[u8]>>, DownloadError> {
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| DownloadError::Image)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    Ok(reader)
}

fn transient_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

fn transient_error(error: &HttpError) -> bool {
    matches!(
        error,
        HttpError::ConnectTimeout | HttpError::BodyTimeout | HttpError::Transport
    )
}

fn map_api(error: crate::ApiError) -> DownloadError {
    match error {
        crate::ApiError::Http(error) => map_http(error),
        crate::ApiError::Session(error) => DownloadError::Session(error),
        _ => DownloadError::Unauthorized,
    }
}

fn map_http(error: HttpError) -> DownloadError {
    match error {
        HttpError::ResponseTooLarge { max } => DownloadError::ResponseTooLarge { max },
        HttpError::ConnectTimeout => DownloadError::ConnectTimeout,
        HttpError::BodyTimeout => DownloadError::BodyTimeout,
        HttpError::UnsafeOrigin => DownloadError::UnsafeOrigin,
        _ => DownloadError::Transport,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_model_note_names_only_the_host() {
        let url =
            Url::parse("https://files.example/secret/path?signature=hidden#part").expect("address");
        let note = refused_model_note(&url);
        assert!(note.contains("the host files.example."), "{note}");
        for hidden in ["secret", "signature", "hidden", "part"] {
            assert!(!note.contains(hidden), "{note}");
        }
    }
}
