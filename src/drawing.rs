use image::{DynamicImage, ImageFormat, Pixel, Rgb, RgbImage, Rgba, RgbaImage};
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn calculate_bounding_box(ways: &[&[(Vec<(f64, f64)>, u32)]]) -> (f64, f64, f64, f64) {
    ways.iter()
        .flat_map(|way_group| way_group.iter().flat_map(|(way, _)| way))
        .fold(
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
            |(min_lon, min_lat, max_lon, max_lat), &(lon, lat)| {
                (
                    min_lon.min(lon),
                    min_lat.min(lat),
                    max_lon.max(lon),
                    max_lat.max(lat),
                )
            },
        )
}

fn lon_lat_to_pixel(
    lon: f64,
    lat: f64,
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    img_size: u32,
) -> (i32, i32) {
    let lon_range = max_lon - min_lon;
    let lat_range = max_lat - min_lat;

    let x = ((lon - min_lon) / lon_range * img_size as f64) as i32;
    let y = ((max_lat - lat) / lat_range * img_size as f64) as i32;

    (x, y)
}

fn plot(img: &mut RgbaImage, x: i32, y: i32, color: Rgba<u8>, alpha: f32) {
    if x >= 0 && y >= 0 && x < img.width() as i32 && y < img.height() as i32 {
        let pixel = img.get_pixel_mut(x as u32, y as u32);
        *pixel = interpolate(*pixel, color, alpha);
    }
}

fn draw_line_wu(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba<u8>) {
    let (dx, dy) = ((x1 - x0).abs(), (y1 - y0).abs());
    let (mut x0, mut y0, mut x1, mut y1) = if dy > dx {
        (y0, x0, y1, x1)
    } else {
        (x0, y0, x1, y1)
    };

    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
        std::mem::swap(&mut y0, &mut y1);
    }

    let gradient = if x1 - x0 == 0 {
        1.0
    } else {
        (y1 - y0) as f32 / (x1 - x0) as f32
    };
    let mut intery = (y0 as f32) + gradient * 0.5;

    for x in x0..=x1 {
        if dy > dx {
            plot(
                img,
                intery.floor() as i32,
                x,
                color,
                1.0 - (intery - intery.floor()),
            );
            plot(
                img,
                intery.floor() as i32 + 1,
                x,
                color,
                intery - intery.floor(),
            );
        } else {
            plot(
                img,
                x,
                intery.floor() as i32,
                color,
                1.0 - (intery - intery.floor()),
            );
            plot(
                img,
                x,
                intery.floor() as i32 + 1,
                color,
                intery - intery.floor(),
            );
        }
        intery += gradient;
    }
}

pub fn draw_map(
    highways: &[(Vec<(f64, f64)>, u32)],
    waterways: &[(Vec<(f64, f64)>, u32)],
    railways: &[(Vec<(f64, f64)>, u32)],
    path: &[(f64, f64)],
) {
    if highways.is_empty() && waterways.is_empty() && railways.is_empty() {
        println!("No ways to draw.");
        return;
    }

    let (min_lon, min_lat, max_lon, max_lat) =
        calculate_bounding_box(&[highways, waterways, railways]);

    println!(
        "Bounding box: ({}, {}), ({}, {})",
        min_lon, min_lat, max_lon, max_lat
    );

    let tiles_x = 5;
    let tiles_y = 5;
    let img_size: u32 = 4096 * 2;
    let subgroups = 2;

    let lon_step = (max_lon - min_lon) / tiles_x as f64;
    let lat_step = (max_lat - min_lat) / tiles_y as f64;

    let tiles: Vec<(usize, usize)> = (0..tiles_x)
        .flat_map(|x| (0..tiles_y).map(move |y| (x, y)))
        .collect();

    tiles.par_iter().for_each(|&(x, y)| {
        let time_start = Instant::now();
        let tile_min_lon = min_lon + x as f64 * lon_step;
        let tile_max_lon = tile_min_lon + lon_step;
        let tile_min_lat = min_lat + y as f64 * lat_step;
        let tile_max_lat = tile_min_lat + lat_step;

        let mut img: image::ImageBuffer<Rgba<u8>, Vec<u8>> = RgbaImage::new(img_size, img_size);

        draw_ways(
            &mut img,
            highways,
            tile_min_lon,
            tile_min_lat,
            tile_max_lon,
            tile_max_lat,
            img_size,
            Rgba([255,255,255,255]),
        );
        draw_ways(
            &mut img,
            waterways,
            tile_min_lon,
            tile_min_lat,
            tile_max_lon,
            tile_max_lat,
            img_size,
            Rgba([0, 0, 255, 255]),
        );
        draw_ways(
            &mut img,
            railways,
            tile_min_lon,
            tile_min_lat,
            tile_max_lon,
            tile_max_lat,
            img_size,
            Rgba([255, 0, 0, 255]),
        );

        draw_path(
            &mut img,
            path,
            tile_min_lon,
            tile_min_lat,
            tile_max_lon,
            tile_max_lat,
            img_size,
            Rgba([0, 255, 0, 255]),
            4, // Added parameter for line thickness
        );

        let file_name = format!("osm_map_{}_{}.png", x, y);
        img.save(&file_name).unwrap();
        println!("Tile {}_{} rendered in {:?}", x, y, time_start.elapsed());
    });

    stitch_images(tiles_x, tiles_y, img_size, "osm_map", "stitched_map.png");
}
fn draw_ways(
    img: &mut RgbaImage,
    ways: &[(Vec<(f64, f64)>, u32)],
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    img_size: u32,
    color: Rgba<u8>,
) {
    for (way, _width) in ways {
        for w in way.windows(2) {
            let (x0, y0) =
                lon_lat_to_pixel(w[0].0, w[0].1, min_lon, min_lat, max_lon, max_lat, img_size);
            let (x1, y1) =
                lon_lat_to_pixel(w[1].0, w[1].1, min_lon, min_lat, max_lon, max_lat, img_size);

            if x0 >= 0
                && x0 < img_size as i32
                && y0 >= 0
                && y0 < img_size as i32
                && x1 >= 0
                && x1 < img_size as i32
                && y1 >= 0
                && y1 < img_size as i32
            {
                draw_line_wu(img, x0, y0, x1, y1, color);
            }
        }
    }
}

fn interpolate(c1: Rgba<u8>, c2: Rgba<u8>, t: f32) -> Rgba<u8> {
    c1.map2(&c2, |a, b| {
        let a = a as f32;
        let b = b as f32;
        (a * (1.0 - t) + b * t) as u8
    })
}

fn draw_path(
    img: &mut RgbaImage,
    path: &[(f64, f64)],
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    img_size: u32,
    color: Rgba<u8>,
    thickness: i32, // Added parameter for line thickness
) {
    for points in path.windows(2) {
        let (x0, y0) = lon_lat_to_pixel(
            points[0].0,
            points[0].1,
            min_lon,
            min_lat,
            max_lon,
            max_lat,
            img_size,
        );
        let (x1, y1) = lon_lat_to_pixel(
            points[1].0,
            points[1].1,
            min_lon,
            min_lat,
            max_lon,
            max_lat,
            img_size,
        );

        // Draw lines offset by a certain amount perpendicular to the line direction
        for offset in -thickness..=thickness {
            let (offset_x, offset_y) = perpendicular_offset(x0, y0, x1, y1, offset);
            if (x0 + offset_x).abs() < img_size as i32
                && (y0 + offset_y).abs() < img_size as i32
                && (x1 + offset_x).abs() < img_size as i32
                && (y1 + offset_y).abs() < img_size as i32
            {
                draw_line_wu(
                    img,
                    x0 + offset_x,
                    y0 + offset_y,
                    x1 + offset_x,
                    y1 + offset_y,
                    color,
                );
            }
        }
    }
}

fn perpendicular_offset(x0: i32, y0: i32, x1: i32, y1: i32, offset: i32) -> (i32, i32) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    if dx == 0 && dy == 0 {
        return (0, 0); // No offset needed for zero-length line
    }
    let length = ((dx * dx + dy * dy) as f64).sqrt();
    let offset_x = (-dy as f64 * offset as f64 / length).round() as i32;
    let offset_y = (dx as f64 * offset as f64 / length).round() as i32;
    (offset_x, offset_y)
}

fn stitch_images(
    tiles_x: usize,
    tiles_y: usize,
    img_size: u32,
    tile_prefix: &str,
    output_file: &str,
) {
    let total_width = img_size * tiles_x as u32;
    let total_height = img_size * tiles_y as u32;

    let mut stitched_image = RgbaImage::new(total_width, total_height);
    // make background black
    for x in 0..total_width {
        for y in 0..total_height {
            stitched_image.put_pixel(x, y, Rgba([0, 0, 0, 255]));
        }
    }
    
    for x in 0..tiles_x {
        for y in 0..tiles_y {
            let start_time = Instant::now();
            let file_name = format!("{}_{}_{}.png", tile_prefix, x, y);
            let tile_image = image::open(&file_name).unwrap().to_rgba8();
            let (width, height) = tile_image.dimensions();

            for tx in 0..width {
                for ty in 0..height {
                    stitched_image.put_pixel(
                        x as u32 * img_size + tx,
                        (tiles_y as u32 - y as u32 - 1) * img_size + ty,
                        *tile_image.get_pixel(tx, ty),
                    );
                }
            }
            println!("Tile {}_{} stitched in {:?}", x, y, start_time.elapsed());
        }
    }

    let fout = &mut BufWriter::new(File::create(Path::new(output_file)).unwrap());
    stitched_image
        .write_to(fout, image::ImageFormat::Png)
        .unwrap();
}
