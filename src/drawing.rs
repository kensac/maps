use plotters::{coord::types::RangedCoordf64, prelude::*};
use image::{GenericImageView, ImageBuffer, RgbaImage};

pub fn draw_map(
    highways: &[(Vec<(f64, f64)>, u32)],
    waterways: &[(Vec<(f64, f64)>, u32)],
    railways: &[(Vec<(f64, f64)>, u32)],
) {
    if highways.is_empty() && waterways.is_empty() && railways.is_empty() {
        println!("No ways to draw.");
        return;
    }

    // Calculate bounding box
    let (min_lon, min_lat, max_lon, max_lat) =
        calculate_bounding_box(&[highways, waterways, railways]);

    println!(
        "Bounding box: ({}, {}), ({}, {})",
        min_lon, min_lat, max_lon, max_lat
    );

    let tiles_x = 20;
    let tiles_y = 20;
    let img_size: u32 = 4096 * 2;

    let lon_step = (max_lon - min_lon) / tiles_x as f64;
    let lat_step = (max_lat - min_lat) / tiles_y as f64;

    for x in 0..tiles_x {
        for y in 0..tiles_y {
            let time_start = std::time::Instant::now();
            let tile_min_lon = min_lon + x as f64 * lon_step;
            let tile_max_lon = tile_min_lon + lon_step;
            let tile_min_lat = min_lat + y as f64 * lat_step;
            let tile_max_lat = tile_min_lat + lat_step;

            let file_name = format!("osm_map_{}_{}.png", x, y);
            let root = BitMapBackend::new(&file_name, (img_size, img_size)).into_drawing_area();
            root.fill(&WHITE).unwrap();

            let mut chart = ChartBuilder::on(&root)
                .build_cartesian_2d(tile_min_lon..tile_max_lon, tile_min_lat..tile_max_lat)
                .unwrap();

            chart.configure_mesh().draw().unwrap();

            draw_way(&mut chart, highways, BLACK);
            draw_way(&mut chart, waterways, BLUE);
            draw_way(&mut chart, railways, RED);

            root.present().unwrap();
            println!("Tile {}_{} rendered in {:?}", x, y, time_start.elapsed());
        }
    }

    stitch_images(tiles_x, tiles_y, img_size, "osm_map", "stitched_map.png");
}

fn draw_way(
    chart: &mut ChartContext<BitMapBackend, Cartesian2d<RangedCoordf64, RangedCoordf64>>,
    ways: &[(Vec<(f64, f64)>, u32)],
    color: RGBColor,
) {
    for (way, width) in ways {
        chart
            .draw_series(LineSeries::new(
                way.iter().cloned(),
                color.stroke_width(*width),
            ))
            .unwrap();
    }
}

fn calculate_bounding_box(ways: &[&[(Vec<(f64, f64)>, u32)]]) -> (f64, f64, f64, f64) {
    let mut min_lon = f64::MAX;
    let mut min_lat = f64::MAX;
    let mut max_lon = f64::MIN;
    let mut max_lat = f64::MIN;

    for way_group in ways {
        for (way, _) in *way_group {
            for &(lon, lat) in way {
                if lon < min_lon {
                    min_lon = lon;
                }
                if lat < min_lat {
                    min_lat = lat;
                }
                if lon > max_lon {
                    max_lon = lon;
                }
                if lat > max_lat {
                    max_lat = lat;
                }
            }
        }
    }

    (min_lon, min_lat, max_lon, max_lat)
}

fn stitch_images(tiles_x: usize, tiles_y: usize, img_size: u32, tile_prefix: &str, output_file: &str) {
    let trimmed_size = img_size; // Trim 4 pixels from each side
    let mut stitched_image: RgbaImage = ImageBuffer::new(trimmed_size * tiles_x as u32, trimmed_size * tiles_y as u32);

    for x in 0..tiles_x {
        for y in 0..tiles_y {
            let time_start = std::time::Instant::now();

            let file_name = format!("{}_{}_{}.png", tile_prefix, x, y);
            let tile_image = image::open(&file_name).unwrap();
            let (width, height) = tile_image.dimensions();

            for tx in 0..(width) {
                for ty in 0..(height) {
                    stitched_image.put_pixel(
                        x as u32 * trimmed_size + (tx),
                        (tiles_y as u32 - y as u32 - 1) * trimmed_size + (ty),
                        tile_image.get_pixel(tx, ty),
                    );
                }
            }
            println!("Tile {}_{} stitched in {:?}", x, y, time_start.elapsed());
        }
    }

    stitched_image.save(output_file).unwrap();
}
