//! Writing rendered pixels to disk: an XYZ tile pyramid with a local viewer,
//! or a single poster image streamed in horizontal bands.

use crate::geo::{unproject, Rect, TILE_SIZE};
use crate::map::Map;
use crate::render::{Renderer, Viewport};
use anyhow::{ensure, Context, Result};
use rayon::prelude::*;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tiny_skia::Pixmap;

const VIEWER: &str = include_str!("viewer.html");

/// Converts premultiplied RGBA to straight RGBA in place.
fn demultiply(data: &mut [u8]) {
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3];
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            }
        }
    }
}

fn encode_png(pixmap: Pixmap) -> Result<Vec<u8>> {
    let (w, h) = (pixmap.width(), pixmap.height());
    let mut data = pixmap.take();
    // Most tiles are fully opaque: drop the alpha channel for them.
    let opaque = data.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
    let color = if opaque {
        let mut write = 0;
        for read in (0..data.len()).step_by(4) {
            data.copy_within(read..read + 3, write);
            write += 3;
        }
        data.truncate(write);
        png::ColorType::Rgb
    } else {
        demultiply(&mut data);
        png::ColorType::Rgba
    };
    let mut out = Vec::with_capacity(data.len() / 4);
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.write_header()?.write_image_data(&data)?;
    Ok(out)
}

/// Tile column/row range covering `bounds` at zoom `z`.
fn tile_range(bounds: &Rect, z: u8) -> (u32, u32, u32, u32) {
    let n = 1u32 << z;
    let to_tile = |v: f64| ((v * n as f64).floor().max(0.0) as u32).min(n - 1);
    (
        to_tile(bounds.min_x),
        to_tile(bounds.min_y),
        to_tile(bounds.max_x),
        to_tile(bounds.max_y),
    )
}

/// Total number of tiles in a pyramid.
pub fn count_tiles(bounds: &Rect, min_zoom: u8, max_zoom: u8) -> u64 {
    (min_zoom..=max_zoom)
        .map(|z| {
            let (x0, y0, x1, y1) = tile_range(bounds, z);
            (x1 - x0 + 1) as u64 * (y1 - y0 + 1) as u64
        })
        .sum()
}

/// Renders `{z}/{x}/{y}.png` for every tile covering the map in the zoom
/// range, plus an `index.html` that browses them.
pub fn write_tiles(
    renderer: &Renderer,
    map: &Map,
    dir: &Path,
    min_zoom: u8,
    max_zoom: u8,
    scale: f32,
) -> Result<()> {
    ensure!(
        min_zoom <= max_zoom,
        "--min-zoom must not exceed --max-zoom"
    );
    ensure!(max_zoom <= 22, "zoom levels above 22 are not supported");
    let total = count_tiles(&map.bounds, min_zoom, max_zoom);
    eprintln!(
        "rendering {total} tiles, z{min_zoom}–z{max_zoom}, into {}",
        dir.display()
    );
    let started = Instant::now();
    let bytes = AtomicU64::new(0);

    for z in min_zoom..=max_zoom {
        let t = Instant::now();
        let (x0, y0, x1, y1) = tile_range(&map.bounds, z);
        for x in x0..=x1 {
            fs::create_dir_all(dir.join(format!("{z}/{x}")))?;
        }
        let tiles: Vec<(u32, u32)> = (x0..=x1)
            .flat_map(|x| (y0..=y1).map(move |y| (x, y)))
            .collect();
        tiles.par_iter().try_for_each(|&(x, y)| -> Result<()> {
            let vp = Viewport::tile(z, x, y, scale);
            let png = encode_png(renderer.render(&vp))?;
            bytes.fetch_add(png.len() as u64, Ordering::Relaxed);
            let path = dir.join(format!("{z}/{x}/{y}.png"));
            fs::write(&path, png).with_context(|| format!("writing {}", path.display()))
        })?;
        let secs = t.elapsed().as_secs_f64();
        eprintln!(
            "  z{z:<2} {:>7} tiles in {:>7.2}s ({:>6.0} tiles/s)",
            tiles.len(),
            secs,
            tiles.len() as f64 / secs.max(1e-9)
        );
    }

    let (west, north) = unproject([map.bounds.min_x, map.bounds.min_y]);
    let (east, south) = unproject([map.bounds.max_x, map.bounds.max_y]);
    let html = VIEWER
        .replace("{{MIN_ZOOM}}", &min_zoom.to_string())
        .replace("{{MAX_ZOOM}}", &max_zoom.to_string())
        .replace(
            "{{BOUNDS}}",
            &format!("[[{south}, {west}], [{north}, {east}]]"),
        );
    fs::write(dir.join("index.html"), html)?;
    eprintln!(
        "done: {total} tiles, {:.1} MiB in {:.2?}; open {}",
        bytes.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0),
        started.elapsed(),
        dir.join("index.html").display()
    );
    Ok(())
}

/// How large a poster to render.
#[derive(Clone, Copy, Debug)]
pub enum PosterSize {
    /// Output width in pixels; zoom follows from it.
    Width(u32),
    /// Style zoom; output size follows from it.
    Zoom(f64),
}

/// Renders `region` (normalized Mercator) into one PNG. The image is drawn in
/// horizontal bands on all cores and streamed to the encoder in order, so
/// memory stays bounded even for gigapixel output.
pub fn write_poster(
    renderer: &Renderer,
    region: Rect,
    size: PosterSize,
    scale: f32,
    path: &Path,
) -> Result<()> {
    let tile = TILE_SIZE * scale as f64;
    let (world_px, zoom) = match size {
        PosterSize::Width(w) => {
            let world = w as f64 / region.width();
            (world, (world / tile).log2())
        }
        PosterSize::Zoom(z) => (tile * z.exp2(), z),
    };
    let width = (region.width() * world_px).round() as u32;
    let height = (region.height() * world_px).round() as u32;
    ensure!(width > 0 && height > 0, "empty output region");
    ensure!(
        (width as u64) * (height as u64) <= 4_000_000_000,
        "{width}×{height} is too large; lower --width or --zoom"
    );
    eprintln!(
        "rendering {width}×{height} px at zoom {zoom:.2} into {}",
        path.display()
    );

    let t = Instant::now();
    let file = fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;

    // Bands of roughly 16 MiB each, a few per core in flight at once. PNG
    // encoding is sequential, so it runs on its own thread and overlaps with
    // rendering the next group of bands.
    let band_rows = ((16usize << 20) / (width as usize * 4)).clamp(16, 1024) as u32;
    let bands: Vec<u32> = (0..height).step_by(band_rows as usize).collect();
    let in_flight = rayon::current_num_threads() * 2;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<Vec<u8>>>(1);

    std::thread::scope(|scope| -> Result<()> {
        let encoder = scope.spawn(move || -> Result<()> {
            let mut enc = png::Encoder::new(BufWriter::with_capacity(1 << 22, file), width, height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            let mut writer = enc.write_header()?;
            let mut stream = writer.stream_writer()?;
            for group in rx {
                for band in group {
                    stream.write_all(&band)?;
                }
            }
            stream.finish()?;
            writer.finish()?;
            Ok(())
        });
        for group in bands.chunks(in_flight) {
            let rendered: Vec<Vec<u8>> = group
                .par_iter()
                .map(|&top| {
                    let vp = Viewport {
                        x: region.min_x * world_px,
                        y: region.min_y * world_px + top as f64,
                        width,
                        height: band_rows.min(height - top),
                        zoom,
                        scale,
                    };
                    let mut data = renderer.render(&vp).take();
                    demultiply(&mut data);
                    data
                })
                .collect();
            if tx.send(rendered).is_err() {
                break; // The encoder failed; its error is reported below.
            }
        }
        drop(tx);
        encoder.join().expect("encoder thread panicked")
    })?;
    eprintln!(
        "done in {:.2?} ({:.1} Mpx/s)",
        t.elapsed(),
        width as f64 * height as f64 / 1e6 / t.elapsed().as_secs_f64()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ranges_cover_bounds() {
        let r = Rect::from_lon_lat(-74.05, 40.6, -73.9, 40.85);
        let (x0, y0, x1, y1) = tile_range(&r, 12);
        assert!(x0 <= x1 && y0 <= y1);
        assert!(x0 as f64 / 4096.0 <= r.min_x && (x1 + 1) as f64 / 4096.0 >= r.max_x);
        assert_eq!(count_tiles(&r, 0, 0), 1);
    }

    #[test]
    fn demultiply_restores_straight_alpha() {
        let mut px = [64, 32, 0, 128];
        demultiply(&mut px);
        assert_eq!(px, [128, 64, 0, 128]);
    }
}
