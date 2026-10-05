use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use maps::geo::{unproject, Rect};
use maps::map::Map;
use maps::output::{count_tiles, write_poster, write_tiles, PosterSize};
use maps::render::Renderer;
use maps::style::ThemeName;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Render OpenStreetMap extracts (.osm.pbf) into map tiles and posters.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render one large image of the whole extract (or a part of it).
    Render {
        #[command(flatten)]
        common: Common,
        /// Output PNG path.
        #[arg(short, long, default_value = "map.png")]
        output: PathBuf,
        /// Output width in pixels (height follows the region's aspect ratio).
        #[arg(short, long, default_value_t = 4096, conflicts_with = "zoom")]
        width: u32,
        /// Render at this zoom level instead of a fixed width.
        #[arg(short, long)]
        zoom: Option<f64>,
        /// Region to render as west,south,east,north in degrees.
        #[arg(long, value_parser = parse_bbox, allow_hyphen_values = true)]
        bbox: Option<Rect>,
    },
    /// Render an XYZ tile pyramid with an index.html viewer.
    Tiles {
        #[command(flatten)]
        common: Common,
        /// Output directory.
        #[arg(short, long, default_value = "tiles")]
        output: PathBuf,
        #[arg(long, default_value_t = 10)]
        min_zoom: u8,
        #[arg(long, default_value_t = 16)]
        max_zoom: u8,
    },
    /// Print statistics about an extract.
    Info {
        /// Input .osm.pbf file.
        input: PathBuf,
    },
}

#[derive(Args)]
struct Common {
    /// Input .osm.pbf file.
    input: PathBuf,
    /// Color theme.
    #[arg(short, long, value_enum, default_value_t)]
    theme: ThemeName,
    /// Pixel ratio; 2 renders crisp output for high-DPI screens.
    #[arg(short, long, default_value_t = 1.0)]
    scale: f32,
}

fn parse_bbox(s: &str) -> Result<Rect, String> {
    let v: Vec<f64> = s
        .split(',')
        .map(|p| p.trim().parse::<f64>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    match v[..] {
        [w, s, e, n] if w < e && s < n => Ok(Rect::from_lon_lat(w, s, e, n)),
        _ => Err("expected west,south,east,north with west < east and south < north".into()),
    }
}

fn load(path: &Path) -> Result<Map> {
    if !path.exists() {
        bail!("{} does not exist", path.display());
    }
    let t = Instant::now();
    eprintln!("loading {}", path.display());
    let map = Map::build(maps::ingest::read(path)?);
    eprintln!("loaded in {:.2?}", t.elapsed());
    Ok(map)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // Headroom for deep work-stealing recursion over large tile sets.
    rayon::ThreadPoolBuilder::new()
        .stack_size(8 << 20)
        .build_global()?;
    match cli.command {
        Command::Render {
            common,
            output,
            width,
            zoom,
            bbox,
        } => {
            let map = load(&common.input)?;
            let region = match bbox {
                Some(b) => b.intersection(&map.bounds),
                None => map.bounds,
            };
            if region.is_empty() {
                bail!("--bbox does not overlap the extract");
            }
            let size = zoom.map_or(PosterSize::Width(width), PosterSize::Zoom);
            let renderer = Renderer::new(&map, common.theme.theme());
            write_poster(&renderer, region, size, common.scale, &output)
        }
        Command::Tiles {
            common,
            output,
            min_zoom,
            max_zoom,
        } => {
            let map = load(&common.input)?;
            let renderer = Renderer::new(&map, common.theme.theme());
            write_tiles(&renderer, &map, &output, min_zoom, max_zoom, common.scale)
        }
        Command::Info { input } => {
            let map = load(&input)?;
            let (w, n) = unproject([map.bounds.min_x, map.bounds.min_y]);
            let (e, s) = unproject([map.bounds.max_x, map.bounds.max_y]);
            println!("bounds      {w:.5},{s:.5},{e:.5},{n:.5}");
            println!("background  {:?}", map.background);
            println!("features    {}", map.features.len());
            println!("vertices    {}", map.points.len());
            for z in [12, 14, 16, 18] {
                println!("tiles z0–{z:<2} {}", count_tiles(&map.bounds, 0, z));
            }
            println!();
            for (kind, count) in map.stats() {
                println!("{:>10}  {kind:?}", count);
            }
            Ok(())
        }
    }
}
