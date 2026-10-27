//! Asynchronous display assets, independent of the Bluetooth control queue.
use crate::{
    gui_session::Session,
    gui_theme,
    product_assets::{AssetStore, Catalog, ModelKey},
    product_media::Media,
};
use bluetooth::{CancellationToken, DeviceId};
use gpui_kit::{prelude::*, *};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, sync::Semaphore};

const MEMORY_LIMIT: u64 = 256 * 1024 * 1024;
const RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone, Hash, PartialEq, Eq)]
struct MediaKey {
    url: String,
    animated: bool,
}
enum Entry {
    Loading,
    Ready { media: Arc<Media>, used: u64 },
    Failed(Instant),
}
struct Playback {
    token: (String, u64),
    url: String,
    started: Instant,
}
struct Images {
    runtime: Handle,
    cancel: CancellationToken,
    store: Option<Arc<AssetStore>>,
    catalog: Option<Catalog>,
    catalog_loading: bool,
    catalog_failed: Option<Instant>,
    entries: HashMap<MediaKey, Entry>,
    clock: u64,
    playback: Option<Playback>,
    downloads: Arc<Semaphore>,
    preview: bool,
    preview_image: Option<Arc<Media>>,
    preview_animation: Option<Arc<Media>>,
}
impl Global for Images {}

/// Replay the next visible home-page animation using the decoded cache.
pub(super) fn restart(cx: &mut App) {
    if cx.has_global::<Images>() {
        cx.update_global::<Images, _>(|images, _| images.playback = None);
    }
}

pub(super) fn init(runtime: Handle, cancel: CancellationToken, preview: bool, cx: &mut App) {
    cx.set_global(Images {
        runtime,
        cancel,
        store: None,
        catalog: None,
        catalog_loading: false,
        catalog_failed: None,
        entries: HashMap::new(),
        clock: 0,
        playback: None,
        downloads: Arc::new(Semaphore::new(2)),
        preview,
        preview_image: None,
        preview_animation: None,
    });
    cx.update_global::<Images, _>(|images, cx| {
        if preview {
            // Explicit local fixtures retain preview's no-network/no-config contract.
            for (flag, animated) in [
                ("--preview-product-image=", false),
                ("--preview-product-animation=", true),
            ] {
                if let Some(path) =
                    std::env::args().find_map(|arg| arg.strip_prefix(flag).map(PathBuf::from))
                {
                    let task = images.runtime.spawn_blocking(move || {
                        let bytes = std::fs::read(path)?;
                        Media::decode(&bytes, animated).map(Arc::new)
                    });
                    cx.spawn(async move |cx| {
                        if let Ok(Ok(media)) = task.await {
                            cx.update_global::<Images, _>(|images, cx| {
                                if animated {
                                    images.preview_animation = Some(media);
                                } else {
                                    images.preview_image = Some(media);
                                }
                                cx.refresh_windows();
                            });
                        }
                    })
                    .detach();
                }
            }
        } else {
            images.load_catalog(cx);
        }
    });
}

/// Use protocol identity only. A Bluetooth alias is not a model/color identity.
pub(super) fn model_key(session: &Session, device: Option<&DeviceId>) -> Option<ModelKey> {
    let device = device?;
    if let Some(snapshot) = session.snapshot.as_ref().filter(|s| &s.device == device)
        && let (Some(name), Some(appearance)) = (&snapshot.model_name, snapshot.appearance)
        && let Ok(name) = name.as_utf8()
    {
        return Some(ModelKey::new(name, appearance.color.as_raw()));
    }
    let saved = session.saved_devices.find(device)?;
    Some(ModelKey::new(
        saved.model_name.as_deref()?,
        saved.model_color?,
    ))
}

/// The custom source schedules work during layout and supplies the embedded
/// placeholder immediately. GPUI only receives single-frame RenderImages.
pub(super) fn source(
    key: Option<ModelKey>,
    playback: Option<(String, u64)>,
    cx: &App,
) -> ImageSource {
    let fallback = Resource::Embedded(gui_theme::product_image(cx).into());
    ImageSource::Custom(Arc::new(move |window, cx| {
        let media = if cx.has_global::<Images>() {
            cx.update_global::<Images, _>(|images, cx| {
                images.resolve(key.as_ref(), playback.as_ref(), window, cx)
            })
        } else {
            None
        };
        media
            .map(Ok)
            .or_else(|| window.use_asset::<ImgResourceLoader>(&fallback, cx))
    }))
}

impl Images {
    fn load_catalog(&mut self, cx: &mut App) {
        if self.preview
            || self.catalog_loading
            || self.catalog.is_some()
            || self
                .catalog_failed
                .is_some_and(|at| at.elapsed() < RETRY_DELAY)
        {
            return;
        }
        self.catalog_loading = true;
        let cancel = self.cancel.clone();
        let task = self.runtime.spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => anyhow::bail!("Application closed"),
                result = async {
                    let store = Arc::new(AssetStore::new()?);
                    let catalog = store.catalog().await?;
                    Ok((store, catalog))
                } => result,
            }
        });
        cx.spawn(async move |cx| {
            let result = task.await;
            cx.update_global::<Images, _>(|images, cx| {
                images.catalog_loading = false;
                match result {
                    Ok(Ok((store, catalog))) => {
                        images.store = Some(store);
                        images.catalog = Some(catalog);
                        images.catalog_failed = None;
                    }
                    _ => images.catalog_failed = Some(Instant::now()),
                }
                cx.refresh_windows();
            });
        })
        .detach();
    }

    fn resolve(
        &mut self,
        key: Option<&ModelKey>,
        token: Option<&(String, u64)>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        if self.preview {
            if let Some(token) = token
                && let Some(media) = self.preview_animation.clone()
            {
                return Some(self.animate(media, "preview", token, window, cx));
            }
            return self.preview_image.as_ref().map(|media| media.first_frame());
        }
        self.load_catalog(cx);
        let urls = self.catalog.as_ref()?.lookup(key?)?.clone();
        let still = self.media(
            MediaKey {
                url: urls.image,
                animated: false,
            },
            cx,
        );
        if let Some(token) = token
            && let Some(url) = urls.animation
            && let Some(media) = self.media(
                MediaKey {
                    url: url.clone(),
                    animated: true,
                },
                cx,
            )
        {
            return Some(self.animate(media, &url, token, window, cx));
        }
        still.map(|media| media.first_frame())
    }

    fn animate(
        &mut self,
        media: Arc<Media>,
        url: &str,
        token: &(String, u64),
        window: &mut Window,
        cx: &App,
    ) -> Arc<RenderImage> {
        let playback = self.playback.get_or_insert_with(|| Playback {
            token: token.clone(),
            url: url.to_owned(),
            started: Instant::now(),
        });
        if playback.token != *token || playback.url != url {
            *playback = Playback {
                token: token.clone(),
                url: url.to_owned(),
                started: Instant::now(),
            };
        }
        let elapsed = if cx.reduce_motion() {
            media.duration()
        } else {
            playback.started.elapsed()
        };
        let (frame, next) = media.frame_at(elapsed);
        if next.is_some() && window.is_window_active() {
            window.request_animation_frame();
        }
        frame
    }

    fn media(&mut self, key: MediaKey, cx: &mut App) -> Option<Arc<Media>> {
        self.clock = self.clock.wrapping_add(1);
        match self.entries.get_mut(&key) {
            Some(Entry::Ready { media, used }) => {
                *used = self.clock;
                return Some(media.clone());
            }
            Some(Entry::Loading) => return None,
            Some(Entry::Failed(at)) if at.elapsed() < RETRY_DELAY => return None,
            _ => {}
        }
        let store = self.store.clone()?;
        self.entries.insert(key.clone(), Entry::Loading);
        let request = key.clone();
        let cancel = self.cancel.clone();
        let downloads = self.downloads.clone();
        let task = self.runtime.spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => anyhow::bail!("Application closed"),
                result = async {
                    let _permit = downloads.acquire().await?;
                    // A corrupt decoded cache entry gets one fresh download.
                    for attempt in 0..2 {
                        let bytes = store.load_asset(&request.url).await?;
                        let animated = request.animated;
                        let decoded = tokio::task::spawn_blocking(move || Media::decode(&bytes, animated)).await?;
                        match decoded {
                            Ok(media) => return Ok(Arc::new(media)),
                            Err(error) if attempt == 1 => return Err(error),
                            Err(_) => store.invalidate_asset(&request.url).await?,
                        }
                    }
                    unreachable!()
                } => result,
            }
        });
        cx.spawn(async move |cx| {
            let result = task.await;
            cx.update_global::<Images, _>(|images, cx| {
                let entry = match result {
                    Ok(Ok(media)) => Entry::Ready {
                        media,
                        used: images.clock,
                    },
                    _ => Entry::Failed(Instant::now()),
                };
                images.entries.insert(key.clone(), entry);
                images.trim(&key);
                cx.refresh_windows();
            });
        })
        .detach();
        None
    }

    fn trim(&mut self, keep: &MediaKey) {
        loop {
            let bytes: u64 = self
                .entries
                .values()
                .filter_map(|e| match e {
                    Entry::Ready { media, .. } => Some(media.decoded_bytes()),
                    _ => None,
                })
                .sum();
            if bytes <= MEMORY_LIMIT {
                break;
            }
            let oldest = self
                .entries
                .iter()
                .filter_map(|(key, entry)| match entry {
                    Entry::Ready { used, .. } if key != keep => Some((key.clone(), *used)),
                    _ => None,
                })
                .min_by_key(|(_, used)| *used);
            let Some((oldest, _)) = oldest else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }
}
