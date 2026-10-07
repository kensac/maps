//! Public JSON API under `/v1`: feature queries as GeoJSON, point
//! lookups, stats and static renders. The spec is served at
//! `/v1/openapi.json`.

use crate::classify::{flags, Kind, RoofShape};
use crate::geo::{
    meters_per_unit, point_in_ring, project, signed_area2, unproject, Point, TILE_SIZE,
};
use crate::map::{Feature, Map};
use crate::output::encode_png;
use crate::render::{Buildings, Renderer, Viewport, MAX_PITCH};
use crate::style::ThemeName;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{oneshot, Semaphore};

const SPEC: &str = include_str!("web/openapi.json");
const DOCS: &str = include_str!("web/docs.html");
/// Largest static image side, in output pixels.
const MAX_SIDE: u32 = 1280;
/// Most features one page returns.
const MAX_LIMIT: usize = 1000;
/// Widest bbox side for feature queries, in degrees.
const MAX_SPAN: f64 = 0.05;
/// Static renders in flight at once.
const RENDER_SLOTS: usize = 4;

pub struct Api {
    map: &'static Map,
    version: String,
    kinds: BTreeMap<String, Kind>,
    renders: Semaphore,
}

type Shared = Arc<Api>;

pub fn router(map: &'static Map, version: String) -> Router {
    let kinds = map
        .stats()
        .into_iter()
        .map(|(k, _)| (kind_name(k), k))
        .collect();
    let api = Arc::new(Api {
        map,
        version,
        kinds,
        renders: Semaphore::new(RENDER_SLOTS),
    });
    Router::new()
        .route("/v1", get(|| async { docs() }))
        .route("/v1/docs", get(|| async { docs() }))
        .route("/v1/openapi.json", get(|| async { spec() }))
        .route("/v1/info", get(info))
        .route("/v1/kinds", get(kinds_handler))
        .route("/v1/features", get(features))
        .route("/v1/features/{id}", get(feature))
        .route("/v1/lookup", get(lookup))
        .route("/v1/tallest", get(tallest))
        .route("/v1/static", get(static_map))
        .route("/v1/static.png", get(static_map))
        .with_state(api)
}

/// `{"detail": {"code", "message"}}`; `code` is stable, `message` is prose.
fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    let body = json!({ "detail": { "code": code, "message": message.into() } });
    (status, cors(), Json(body)).into_response()
}

fn bad(message: impl Into<String>) -> Response {
    error(StatusCode::BAD_REQUEST, "INVALID_REQUEST", message)
}

fn cors() -> [(header::HeaderName, &'static str); 1] {
    [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")]
}

fn ok(body: Value, max_age: u32) -> Response {
    let cache = format!("public, max-age={max_age}");
    (
        [
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".to_string()),
            (header::CACHE_CONTROL, cache),
        ],
        Json(body),
    )
        .into_response()
}

fn docs() -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], DOCS).into_response()
}

fn spec() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        SPEC,
    )
        .into_response()
}

/// `Kind::BusStop` to `bus_stop`.
fn kind_name(k: Kind) -> String {
    let mut out = String::new();
    for (i, c) in format!("{k:?}").chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn roof_name(r: RoofShape) -> &'static str {
    match r {
        RoofShape::Flat => "flat",
        RoofShape::Gabled => "gabled",
        RoofShape::Hipped => "hipped",
        RoofShape::Pyramidal => "pyramidal",
        RoofShape::Skillion => "skillion",
        RoofShape::Dome => "dome",
    }
}

fn geometry_type(k: Kind) -> &'static str {
    if k.is_point() {
        "point"
    } else if k.is_area() {
        "polygon"
    } else {
        "line"
    }
}

fn round(v: f64, digits: i32) -> f64 {
    let k = 10f64.powi(digits);
    (v * k).round() / k
}

impl Api {
    fn world(&self, p: [f32; 2]) -> Point {
        [
            p[0] as f64 + self.map.origin[0],
            p[1] as f64 + self.map.origin[1],
        ]
    }

    fn lon_lat(&self, p: [f32; 2]) -> Value {
        let (lon, lat) = unproject(self.world(p));
        json!([round(lon, 7), round(lat, 7)])
    }

    fn local(&self, lon: f64, lat: f64) -> [f32; 2] {
        let p = project(lon, lat);
        [
            (p[0] - self.map.origin[0]) as f32,
            (p[1] - self.map.origin[1]) as f32,
        ]
    }

    fn rings(&self, f: &Feature) -> Vec<Vec<Point>> {
        self.map
            .rings(f)
            .map(|r| r.iter().map(|&p| self.world(p)).collect())
            .collect()
    }

    fn ring_json(&self, ring: &[[f32; 2]], elev: Option<&[f32]>) -> Value {
        Value::Array(
            ring.iter()
                .enumerate()
                .map(|(i, &p)| {
                    let mut c = self.lon_lat(p);
                    if let Some(z) = elev.and_then(|e| e.get(i)) {
                        c.as_array_mut().unwrap().push(json!(round(*z as f64, 2)));
                    }
                    c
                })
                .collect(),
        )
    }

    fn geometry(&self, f: &Feature) -> Value {
        let rings: Vec<&[[f32; 2]]> = self.map.rings(f).collect();
        if f.kind.is_point() {
            return json!({ "type": "Point", "coordinates": self.lon_lat(rings[0][0]) });
        }
        if !f.kind.is_area() {
            let elev = self.map.elevations(f);
            let mut offset = 0;
            let lines: Vec<Value> = rings
                .iter()
                .map(|r| {
                    let e = elev.map(|e| &e[offset..offset + r.len()]);
                    offset += r.len();
                    self.ring_json(r, e)
                })
                .collect();
            return match lines.len() {
                1 => json!({ "type": "LineString", "coordinates": lines[0] }),
                _ => json!({ "type": "MultiLineString", "coordinates": lines }),
            };
        }
        // Rings winding like the largest one are outers; the rest are holes
        // of the outer that contains them.
        let world = self.rings(f);
        let areas: Vec<f64> = world.iter().map(|r| signed_area2(r)).collect();
        let largest = areas
            .iter()
            .copied()
            .max_by(|a, b| a.abs().total_cmp(&b.abs()))
            .unwrap_or(0.0);
        let outer = |i: usize| areas[i].signum() == largest.signum();
        let mut polys: Vec<Vec<Value>> = Vec::new();
        let mut outers = Vec::new();
        for (i, r) in rings.iter().enumerate() {
            if outer(i) {
                outers.push(i);
                polys.push(vec![self.ring_json(r, None)]);
            }
        }
        for (i, r) in rings.iter().enumerate() {
            if outer(i) || world[i].is_empty() {
                continue;
            }
            let host = outers
                .iter()
                .position(|&o| point_in_ring(world[i][0], &world[o]))
                .unwrap_or(0);
            if let Some(p) = polys.get_mut(host) {
                p.push(self.ring_json(r, None));
            }
        }
        match polys.len() {
            1 => json!({ "type": "Polygon", "coordinates": polys[0] }),
            _ => json!({ "type": "MultiPolygon", "coordinates": polys }),
        }
    }

    fn properties(&self, f: &Feature) -> Value {
        let mut p = json!({
            "kind": kind_name(f.kind),
            "geometry_type": geometry_type(f.kind),
        });
        let o = p.as_object_mut().unwrap();
        if f.layer != 0 {
            o.insert("layer".into(), json!(f.layer));
        }
        for (flag, name) in [
            (flags::BRIDGE, "bridge"),
            (flags::TUNNEL, "tunnel"),
            (flags::PART, "building_part"),
            (flags::ON_ROOF, "on_roof"),
        ] {
            if f.flags & flag != 0 {
                o.insert(name.into(), json!(true));
            }
        }
        if f.kind.is_extrusion() || f.kind.is_point() {
            o.insert("height_m".into(), json!(round(f.height as f64, 2)));
            if f.base > 0.0 {
                o.insert("min_height_m".into(), json!(round(f.base as f64, 2)));
            }
        }
        if f.kind.is_extrusion() {
            let d = self.map.detail(f);
            o.insert("roof_shape".into(), json!(roof_name(d.roof)));
            if d.levels > 0.0 {
                o.insert("levels".into(), json!(d.levels));
            }
            if let Some(c) = d.roof_colour {
                o.insert("roof_colour".into(), json!(format!("#{c:06x}")));
            }
            if let Some(c) = d.facade_colour {
                o.insert("facade_colour".into(), json!(format!("#{c:06x}")));
            }
        }
        if f.heading.is_finite() {
            let deg = f.heading.to_degrees().rem_euclid(360.0);
            o.insert("heading_deg".into(), json!(round(deg as f64, 1)));
        }
        if let Some(e) = self.map.elevations(f) {
            let max = e.iter().copied().fold(0.0f32, f32::max);
            o.insert("max_elevation_m".into(), json!(round(max as f64, 2)));
        }
        p
    }

    fn feature_json(&self, id: u32) -> Value {
        let f = &self.map.features[id as usize];
        json!({
            "type": "Feature",
            "id": id,
            "geometry": self.geometry(f),
            "properties": self.properties(f),
        })
    }

    fn kind_filter(&self, kinds: Option<&str>) -> Result<Option<Vec<Kind>>, String> {
        let Some(list) = kinds.filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        list.split(',')
            .map(|s| {
                self.kinds
                    .get(s.trim())
                    .copied()
                    .ok_or_else(|| format!("unknown kind `{s}`; see /v1/kinds"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// Feature ids whose bounds meet `area` (local units), in draw order.
    fn query(&self, area: [f32; 4]) -> Vec<u32> {
        let mut out = Vec::new();
        self.map.query(area, f32::MAX, &mut out);
        out
    }
}

/// `west,south,east,north` in degrees.
fn parse_bbox(s: &str) -> Result<[f64; 4], String> {
    let v: Vec<f64> = s
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| "bbox must be west,south,east,north".to_string())?;
    let [w, s, e, n] = v[..] else {
        return Err("bbox must have 4 numbers".to_string());
    };
    if !(w < e && s < n && (-180.0..=180.0).contains(&w) && (-85.0..=85.0).contains(&s)) {
        return Err("bbox is empty or out of range".to_string());
    }
    if e - w > MAX_SPAN || n - s > MAX_SPAN {
        return Err(format!("bbox sides must be at most {MAX_SPAN} degrees"));
    }
    Ok([w, s, e, n])
}

fn bbox_area(api: &Api, [w, s, e, n]: [f64; 4]) -> [f32; 4] {
    let a = api.local(w, n);
    let b = api.local(e, s);
    [a[0], a[1], b[0], b[1]]
}

async fn info(State(api): State<Shared>) -> Response {
    let map = api.map;
    let (west, north) = unproject([map.bounds.min_x, map.bounds.min_y]);
    let (east, south) = unproject([map.bounds.max_x, map.bounds.max_y]);
    let (lon, lat) = unproject(map.bounds.center());
    ok(
        json!({
            "version": api.version,
            "bounds": [west, south, east, north],
            "center": [lon, lat],
            "features": map.features.len(),
            "vertices": map.points.len(),
            "kinds": api.kinds.len(),
            "tallest_m": map.max_height,
        }),
        60,
    )
}

async fn kinds_handler(State(api): State<Shared>) -> Response {
    let kinds: Vec<Value> = api
        .map
        .stats()
        .into_iter()
        .map(
            |(k, n)| json!({ "kind": kind_name(k), "geometry_type": geometry_type(k), "count": n }),
        )
        .collect();
    ok(json!({ "kinds": kinds }), 3600)
}

#[derive(Deserialize)]
struct FeaturesQuery {
    bbox: String,
    kind: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
}

async fn features(State(api): State<Shared>, Query(q): Query<FeaturesQuery>) -> Response {
    let bbox = match parse_bbox(&q.bbox) {
        Ok(b) => b,
        Err(e) => return bad(e),
    };
    let kinds = match api.kind_filter(q.kind.as_deref()) {
        Ok(k) => k,
        Err(e) => return bad(e),
    };
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(100).clamp(1, MAX_LIMIT);
    let start = (page - 1).saturating_mul(page_size);
    let map = api.map;
    let ids: Vec<u32> = api
        .query(bbox_area(&api, bbox))
        .into_iter()
        .filter(|&i| {
            let k = map.features[i as usize].kind;
            k != Kind::Land && kinds.as_ref().is_none_or(|ks| ks.contains(&k))
        })
        .collect();
    let features: Vec<Value> = ids
        .iter()
        .skip(start)
        .take(page_size)
        .map(|&i| api.feature_json(i))
        .collect();
    ok(
        json!({
            "type": "FeatureCollection",
            "page": page,
            "page_size": page_size,
            "total": ids.len(),
            "has_more": start.saturating_add(page_size) < ids.len(),
            "features": features,
        }),
        300,
    )
}

async fn feature(State(api): State<Shared>, Path(id): Path<u32>) -> Response {
    match api.map.features.get(id as usize) {
        Some(f) if f.kind != Kind::Land => ok(api.feature_json(id), 300),
        _ => error(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("no feature {id}"),
        ),
    }
}

#[derive(Deserialize)]
struct LookupQuery {
    lat: f64,
    lon: f64,
    radius: Option<f64>,
}

/// Features at a point: areas containing it and lines or points within
/// `radius` meters, tallest first.
async fn lookup(State(api): State<Shared>, Query(q): Query<LookupQuery>) -> Response {
    if !(-85.0..=85.0).contains(&q.lat) || !(-180.0..=180.0).contains(&q.lon) {
        return bad("lat or lon out of range");
    }
    let radius = q.radius.unwrap_or(15.0).clamp(1.0, 100.0);
    let p = api.local(q.lon, q.lat);
    let world = project(q.lon, q.lat);
    let r = (radius / meters_per_unit(world[1])) as f32;
    let map = api.map;
    let mut hits: Vec<(f32, u32)> = Vec::new();
    for i in api.query([p[0] - r, p[1] - r, p[0] + r, p[1] + r]) {
        let f = &map.features[i as usize];
        if f.kind == Kind::Land {
            continue;
        }
        let hit = if f.kind.is_area() {
            let pt = [p[0] as f64, p[1] as f64];
            let inside = map
                .rings(f)
                .filter(|ring| {
                    let ring: Vec<Point> =
                        ring.iter().map(|q| [q[0] as f64, q[1] as f64]).collect();
                    point_in_ring(pt, &ring)
                })
                .count();
            inside % 2 == 1
        } else {
            map.rings(f).any(|ring| near(ring, p, r))
        };
        if hit {
            hits.push((f.height, i));
        }
    }
    hits.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.truncate(50);
    let features: Vec<Value> = hits.iter().map(|&(_, i)| api.feature_json(i)).collect();
    ok(
        json!({ "type": "FeatureCollection", "features": features }),
        300,
    )
}

/// Whether any segment of `ring` passes within `r` of `p`.
fn near(ring: &[[f32; 2]], p: [f32; 2], r: f32) -> bool {
    if ring.len() == 1 {
        let d = [ring[0][0] - p[0], ring[0][1] - p[1]];
        return d[0].hypot(d[1]) <= r;
    }
    ring.windows(2).any(|w| {
        let (a, b) = (w[0], w[1]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1];
        let t = if len2 > 0.0 {
            (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = [a[0] + ab[0] * t - p[0], a[1] + ab[1] * t - p[1]];
        d[0].hypot(d[1]) <= r
    })
}

#[derive(Deserialize)]
struct TallestQuery {
    bbox: String,
    limit: Option<usize>,
}

/// Tallest buildings and structures in a bbox.
async fn tallest(State(api): State<Shared>, Query(q): Query<TallestQuery>) -> Response {
    let bbox = match parse_bbox(&q.bbox) {
        Ok(b) => b,
        Err(e) => return bad(e),
    };
    let limit = q.limit.unwrap_or(10).clamp(1, 100);
    let map = api.map;
    let mut ids: Vec<u32> = api
        .query(bbox_area(&api, bbox))
        .into_iter()
        .filter(|&i| map.features[i as usize].kind.is_extrusion())
        .collect();
    ids.sort_by(|&a, &b| {
        let (fa, fb) = (&map.features[a as usize], &map.features[b as usize]);
        fb.height.total_cmp(&fa.height).then(a.cmp(&b))
    });
    ids.truncate(limit);
    let features: Vec<Value> = ids.iter().map(|&i| api.feature_json(i)).collect();
    ok(
        json!({ "type": "FeatureCollection", "features": features }),
        300,
    )
}

#[derive(Deserialize)]
struct StaticQuery {
    lat: f64,
    lon: f64,
    zoom: f64,
    bearing: Option<f64>,
    pitch: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    scale: Option<f32>,
    theme: Option<String>,
    buildings: Option<String>,
    /// Data version from `/meta.json`; a matching one makes the render
    /// cacheable forever, since a new version changes the URL.
    v: Option<String>,
}

/// A PNG of the map centered on a point, at any bearing and pitch.
async fn static_map(State(api): State<Shared>, Query(q): Query<StaticQuery>) -> Response {
    if !(-85.0..=85.0).contains(&q.lat) || !(-180.0..=180.0).contains(&q.lon) {
        return bad("lat or lon out of range");
    }
    if !(0.0..=20.0).contains(&q.zoom) {
        return bad("zoom must be between 0 and 20");
    }
    let scale = q.scale.unwrap_or(1.0);
    if !(1.0..=2.0).contains(&scale) {
        return bad("scale must be between 1 and 2");
    }
    let (w, h) = (q.width.unwrap_or(640), q.height.unwrap_or(400));
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return bad(format!("width and height must be 1 to {MAX_SIDE}"));
    }
    let theme = match q.theme.as_deref().unwrap_or("light") {
        "light" => ThemeName::Light,
        "dark" => ThemeName::Dark,
        _ => return bad("theme must be light or dark"),
    };
    let buildings = match q.buildings.as_deref().unwrap_or("3d") {
        "3d" => Buildings::Extruded,
        "flat" => Buildings::Flat,
        _ => return bad("buildings must be 3d or flat"),
    };
    let (width, height) = (
        (w as f32 * scale).round() as u32,
        (h as f32 * scale).round() as u32,
    );
    let mut vp = Viewport {
        x: 0.0,
        y: 0.0,
        width,
        height,
        zoom: q.zoom,
        scale,
        bearing: q.bearing.unwrap_or(0.0).rem_euclid(360.0),
        pitch: q.pitch.unwrap_or(0.0).clamp(0.0, MAX_PITCH),
        foreshorten: true,
    };
    let world = project(q.lon, q.lat);
    let k = TILE_SIZE * scale as f64 * q.zoom.exp2();
    let c = vp.forward([world[0] * k, world[1] * k]);
    vp.x = c[0] - width as f64 / 2.0;
    vp.y = c[1] - height as f64 / 2.0;

    let Ok(_permit) = api.renders.try_acquire() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "BUSY",
            "too many renders, retry shortly",
        );
    };
    let map = api.map;
    let cache = if q.v.as_deref() == Some(api.version.as_str()) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=60"
    };
    let (tx, rx) = oneshot::channel();
    rayon::spawn(move || {
        if tx.is_closed() {
            return;
        }
        let t = Instant::now();
        let renderer = Renderer::new(map, theme.theme(), buildings);
        let png = encode_png(&renderer.render(&vp));
        let _ = tx.send(png.map(|b| (b, t.elapsed())));
    });
    match rx.await {
        Ok(Ok((png, took))) => Response::builder()
            .header(header::CONTENT_TYPE, "image/png")
            .header(header::CACHE_CONTROL, cache)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(
                "server-timing",
                format!("render;dur={:.1}", took.as_secs_f64() * 1e3),
            )
            .body(Body::from(png))
            .expect("valid response"),
        Ok(Err(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "RENDER_FAILED",
            format!("{e:#}"),
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "RENDER_FAILED",
            "render cancelled",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_are_snake_case() {
        assert_eq!(kind_name(Kind::Building), "building");
        assert_eq!(kind_name(Kind::StreetLamp), "street_lamp");
    }

    #[test]
    fn bbox_parses_and_limits() {
        assert!(parse_bbox("-74.01,40.70,-74.00,40.71").is_ok());
        assert!(parse_bbox("-74,40,-73,41").is_err());
        assert!(parse_bbox("1,2,3").is_err());
        assert!(parse_bbox("-74.00,40.70,-74.01,40.71").is_err());
    }

    #[test]
    fn near_finds_segment() {
        let line = [[0.0, 0.0], [10.0, 0.0]];
        assert!(near(&line, [5.0, 1.0], 1.5));
        assert!(!near(&line, [5.0, 3.0], 1.5));
    }
}
