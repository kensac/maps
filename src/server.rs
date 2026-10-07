//! Real-time tile server.
//!
//! - HTTP on a small tokio runtime, rendering on the rayon pool.
//! - Size-bounded cache; concurrent requests for a tile share one render.
//! - Renders whose clients all left are skipped.
//! - A semaphore caps renders in flight.
//! - Empty tiles are a shared pre-encoded image.
//! - URLs embed a data version so responses cache forever.

use crate::geo::unproject;
use crate::map::Map;
use crate::output::encode_png;
use crate::render::{Buildings, Renderer, Viewport, MAX_PITCH};
use crate::style::ThemeName;
use anyhow::{Context, Result};
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::cell::RefCell;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tiny_skia::Pixmap;
use tokio::sync::{oneshot, Semaphore};

const VIEWER: &str = include_str!("web/gl.html");
const RASTER_VIEWER: &str = include_str!("web/raster.html");
/// Highest zoom the server renders natively; the viewer overzooms beyond it.
pub const MAX_ZOOM: u8 = 20;
const MAX_SCALE: u8 = 3;

pub struct ServeOptions {
    pub addr: SocketAddr,
    /// Tile cache budget in bytes.
    pub cache_bytes: u64,
    /// Opaque token identifying the loaded data, embedded in tile URLs.
    pub version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TileKey {
    theme: ThemeName,
    buildings: Buildings,
    bearing: u16,
    pitch: u8,
    scale: u8,
    z: u8,
    x: i64,
    y: i64,
}

impl TileKey {
    fn viewport(&self) -> Viewport {
        Viewport::camera_tile(
            self.z,
            self.x,
            self.y,
            self.scale as f32,
            self.bearing as f64,
            self.pitch as f64,
        )
    }
}

#[derive(Default)]
struct Metrics {
    requests: AtomicU64,
    hits: AtomicU64,
    rendered: AtomicU64,
    render_nanos: AtomicU64,
    empty: AtomicU64,
    skipped: AtomicU64,
}

/// A geometry tile for the WebGL viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GeoKey {
    theme: ThemeName,
    z: u8,
    x: u32,
    y: u32,
}

struct App {
    map: &'static Map,
    version: String,
    meta: String,
    cache: moka::future::Cache<TileKey, Bytes>,
    geo_cache: moka::future::Cache<GeoKey, Bytes>,
    templates: [OnceLock<Bytes>; 2],
    permits: Semaphore,
    empty: [OnceLock<Bytes>; MAX_SCALE as usize],
    metrics: Metrics,
}

type Shared = Arc<App>;

/// Parses `light`, `dark`, `light-flat`, `dark-flat`.
fn parse_style(s: &str) -> Option<(ThemeName, Buildings)> {
    let (theme, flat) = match s.strip_suffix("-flat") {
        Some(t) => (t, true),
        None => (s, false),
    };
    let theme = match theme {
        "light" => ThemeName::Light,
        "dark" => ThemeName::Dark,
        _ => return None,
    };
    let buildings = if flat {
        Buildings::Flat
    } else {
        Buildings::Extruded
    };
    Some((theme, buildings))
}

/// Parses `12.png`, `12@2x.png`, `-3@3x.png` into `(y, scale)`.
fn parse_file(s: &str) -> Option<(i64, u8)> {
    let stem = s.strip_suffix(".png")?;
    let (y, scale) = match stem.split_once('@') {
        Some((y, sc)) => (y, sc.strip_suffix('x')?.parse::<u8>().ok()?),
        None => (stem, 1),
    };
    (1..=MAX_SCALE)
        .contains(&scale)
        .then_some(())
        .and(y.parse().ok().map(|y| (y, scale)))
}

thread_local! {
    /// One reusable render target per worker thread and tile size.
    static PIXMAP: RefCell<Option<Pixmap>> = const { RefCell::new(None) };
}

fn render_tile(map: &Map, key: TileKey) -> Result<Vec<u8>> {
    let vp = key.viewport();
    let renderer = Renderer::new(map, key.theme.theme(), key.buildings);
    PIXMAP.with(|cell| {
        let mut slot = cell.borrow_mut();
        let pixmap = match slot.as_mut() {
            Some(p) if p.width() == vp.width && p.height() == vp.height => p,
            _ => slot.insert(Pixmap::new(vp.width, vp.height).context("tile size")?),
        };
        renderer.render_into(&vp, pixmap);
        encode_png(pixmap)
    })
}

impl App {
    fn empty_tile(&self, scale: u8) -> Bytes {
        self.empty[scale as usize - 1]
            .get_or_init(|| {
                let size = 256 * scale as u32;
                let pixmap = Pixmap::new(size, size).expect("non-zero tile");
                Bytes::from(encode_png(&pixmap).expect("encoding an empty tile"))
            })
            .clone()
    }

    /// Runs `work` on the rayon pool. Skipped if no one is waiting anymore.
    async fn on_pool<F>(self: Arc<Self>, work: F) -> Result<Bytes, String>
    where
        F: FnOnce(&App) -> Result<Vec<u8>> + Send + 'static,
    {
        let _permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
        let (tx, rx) = oneshot::channel();
        let app = self.clone();
        rayon::spawn(move || {
            if tx.is_closed() {
                app.metrics.skipped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let t = Instant::now();
            let result = work(&app).map(Bytes::from);
            app.metrics
                .render_nanos
                .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            app.metrics.rendered.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
        });
        rx.await.map_err(|_| "render cancelled".to_string())?
    }

    async fn render(self: Arc<Self>, key: TileKey) -> Result<Bytes, String> {
        self.on_pool(move |app| render_tile(app.map, key)).await
    }
}

fn png_response(bytes: Bytes, immutable: bool, cache_status: &'static str) -> Response {
    let cache_control = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=60"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header("x-cache", cache_status)
        .body(Body::from(bytes))
        .expect("valid response")
}

async fn tile(
    State(app): State<Shared>,
    Path((version, style, bearing, pitch, z, x, file)): Path<(
        String,
        String,
        u16,
        u8,
        u8,
        i64,
        String,
    )>,
) -> Response {
    app.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let (Some((theme, buildings)), Some((y, scale))) = (parse_style(&style), parse_file(&file))
    else {
        return (StatusCode::NOT_FOUND, "unknown tile").into_response();
    };
    if z > MAX_ZOOM || pitch as f64 > MAX_PITCH {
        return (StatusCode::NOT_FOUND, "zoom or pitch out of range").into_response();
    }
    let key = TileKey {
        theme,
        buildings,
        bearing: bearing % 360,
        pitch,
        scale,
        z,
        x,
        y,
    };
    let immutable = version == app.version;

    if !Renderer::new(app.map, theme.theme(), buildings).intersects(&key.viewport()) {
        app.metrics.empty.fetch_add(1, Ordering::Relaxed);
        return png_response(app.empty_tile(scale), immutable, "empty");
    }
    if let Some(bytes) = app.cache.get(&key).await {
        app.metrics.hits.fetch_add(1, Ordering::Relaxed);
        return png_response(bytes, immutable, "hit");
    }
    match app.cache.try_get_with(key, app.clone().render(key)).await {
        Ok(bytes) => png_response(bytes, immutable, "miss"),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn page(html: &'static str) -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        html,
    )
}

fn binary(bytes: Bytes, immutable: bool) -> Response {
    let cache_control = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=60"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_ENCODING, "gzip")
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Body::from(bytes))
        .expect("valid response")
}

fn parse_theme(s: &str) -> Option<ThemeName> {
    match s {
        "light" => Some(ThemeName::Light),
        "dark" => Some(ThemeName::Dark),
        _ => None,
    }
}

/// `/geo/{version}/{theme}/{z}/{x}/{y}.bin`: a geometry tile.
async fn geo(
    State(app): State<Shared>,
    Path((version, theme, z, x, file)): Path<(String, String, u8, u32, String)>,
) -> Response {
    let y = file
        .strip_suffix(".bin")
        .and_then(|y| y.parse::<u32>().ok());
    let (Some(theme), Some(y)) = (parse_theme(&theme), y) else {
        return (StatusCode::NOT_FOUND, "unknown tile").into_response();
    };
    if z > crate::vtile::MAX_ZOOM || x >> z != 0 || y >> z != 0 {
        return (StatusCode::NOT_FOUND, "tile out of range").into_response();
    }
    app.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let key = GeoKey { theme, z, x, y };
    let immutable = version == app.version;
    if let Some(bytes) = app.geo_cache.get(&key).await {
        app.metrics.hits.fetch_add(1, Ordering::Relaxed);
        return binary(bytes, immutable);
    }
    let work = app.clone().on_pool(move |app| {
        Ok(crate::vtile::build(
            app.map,
            key.theme.theme(),
            key.z,
            key.x,
            key.y,
        ))
    });
    match app.geo_cache.try_get_with(key, work).await {
        Ok(bytes) => binary(bytes, immutable),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `/models/{version}/{theme}.bin`: object model templates.
async fn models(
    State(app): State<Shared>,
    Path((version, file)): Path<(String, String)>,
) -> Response {
    let Some(theme) = file.strip_suffix(".bin").and_then(parse_theme) else {
        return (StatusCode::NOT_FOUND, "unknown theme").into_response();
    };
    let slot = &app.templates[theme as usize];
    let bytes = slot
        .get_or_init(|| Bytes::from(crate::vtile::templates(theme.theme())))
        .clone();
    binary(bytes, version == app.version)
}

async fn meta(State(app): State<Shared>) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        app.meta.clone(),
    )
}

async fn metrics(State(app): State<Shared>) -> impl IntoResponse {
    app.cache.run_pending_tasks().await;
    let m = &app.metrics;
    let rendered = m.rendered.load(Ordering::Relaxed);
    let nanos = m.render_nanos.load(Ordering::Relaxed);
    let body = format!(
        "# TYPE maps_requests_total counter\nmaps_requests_total {}\n\
         # TYPE maps_cache_hits_total counter\nmaps_cache_hits_total {}\n\
         # TYPE maps_tiles_rendered_total counter\nmaps_tiles_rendered_total {}\n\
         # TYPE maps_render_seconds_total counter\nmaps_render_seconds_total {:.6}\n\
         # TYPE maps_empty_tiles_total counter\nmaps_empty_tiles_total {}\n\
         # TYPE maps_renders_skipped_total counter\nmaps_renders_skipped_total {}\n\
         # TYPE maps_cache_entries gauge\nmaps_cache_entries {}\n\
         # TYPE maps_cache_bytes gauge\nmaps_cache_bytes {}\n",
        m.requests.load(Ordering::Relaxed),
        m.hits.load(Ordering::Relaxed),
        rendered,
        nanos as f64 / 1e9,
        m.empty.load(Ordering::Relaxed),
        m.skipped.load(Ordering::Relaxed),
        app.cache.entry_count(),
        app.cache.weighted_size(),
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}

fn meta_json(map: &Map, version: &str) -> String {
    let (west, north) = unproject([map.bounds.min_x, map.bounds.min_y]);
    let (east, south) = unproject([map.bounds.max_x, map.bounds.max_y]);
    let (lon, lat) = unproject(map.bounds.center());
    let geo = crate::vtile::MAX_ZOOM;
    format!(
        "{{\"version\":\"{version}\",\"bounds\":[{west},{south},{east},{north}],\
         \"center\":[{lon},{lat}],\"maxZoom\":{MAX_ZOOM},\"geoMaxZoom\":{geo},\"maxPitch\":{MAX_PITCH},\"maxScale\":{MAX_SCALE},\
         \"styles\":[\"light\",\"dark\",\"light-flat\",\"dark-flat\"]}}"
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    eprintln!("shutting down");
}

/// Serves `map` until interrupted.
pub fn serve(map: Map, opts: ServeOptions) -> Result<()> {
    let map: &'static Map = Box::leak(Box::new(map));
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
    let app = Arc::new(App {
        map,
        meta: meta_json(map, &opts.version),
        version: opts.version,
        cache: moka::future::Cache::builder()
            .weigher(|_: &TileKey, v: &Bytes| v.len().try_into().unwrap_or(u32::MAX))
            .max_capacity(opts.cache_bytes / 2)
            .build(),
        geo_cache: moka::future::Cache::builder()
            .weigher(|_: &GeoKey, v: &Bytes| v.len().try_into().unwrap_or(u32::MAX))
            .max_capacity(opts.cache_bytes / 2)
            .build(),
        templates: Default::default(),
        permits: Semaphore::new(cpus * 4),
        empty: Default::default(),
        metrics: Metrics::default(),
    });
    let api = crate::api::router(map, app.version.clone());
    let router = Router::new()
        .route("/", get(|| async { page(VIEWER) }))
        .route("/raster", get(|| async { page(RASTER_VIEWER) }))
        .route("/geo/{version}/{theme}/{z}/{x}/{file}", get(geo))
        .route("/models/{version}/{file}", get(models))
        .route("/meta.json", get(meta))
        .route("/health", get(|| async { "ok" }))
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(metrics))
        .route(
            "/tiles/{version}/{style}/{bearing}/{pitch}/{z}/{x}/{file}",
            get(tile),
        )
        .with_state(app)
        .merge(api);

    // HTTP is light work; leave the cores to the rayon render pool.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads((cpus / 4).clamp(2, 8))
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(opts.addr)
            .await
            .with_context(|| format!("binding {}", opts.addr))?;
        eprintln!("serving on http://{}", listener.local_addr()?);
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_parse() {
        assert_eq!(
            parse_style("dark-flat"),
            Some((ThemeName::Dark, Buildings::Flat))
        );
        assert_eq!(
            parse_style("light"),
            Some((ThemeName::Light, Buildings::Extruded))
        );
        assert_eq!(parse_style("neon"), None);
    }

    #[test]
    fn tile_files_parse() {
        assert_eq!(parse_file("12.png"), Some((12, 1)));
        assert_eq!(parse_file("-3@2x.png"), Some((-3, 2)));
        assert_eq!(parse_file("5@9x.png"), None);
        assert_eq!(parse_file("5.jpg"), None);
    }
}
