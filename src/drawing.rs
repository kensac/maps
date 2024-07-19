use plotters::{coord::types::RangedCoordf64, prelude::*};

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

    let root = BitMapBackend::new("osm_map.png", (4096 * 4 * 2, 3072 * 4 * 2)).into_drawing_area();
    root.fill(&WHITE).unwrap();

    let mut chart = ChartBuilder::on(&root)
        .caption("OSM Map", ("sans-serif", 50).into_font())
        .margin(10)
        .x_label_area_size(30)
        .y_label_area_size(30)
        .build_cartesian_2d(min_lon..max_lon, min_lat..max_lat)
        .unwrap();

    chart.configure_mesh().draw().unwrap();

    draw_way(&mut chart, highways, BLACK);
    draw_way(&mut chart, waterways, BLUE);
    draw_way(&mut chart, railways, RED);

    root.present().unwrap();
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

fn calculate_bounding_box(series: &[&[(Vec<(f64, f64)>, u32)]]) -> (f64, f64, f64, f64) {
    let (mut min_lon, mut min_lat) = (f64::INFINITY, f64::INFINITY);
    let (mut max_lon, mut max_lat) = (f64::NEG_INFINITY, f64::NEG_INFINITY);

    for ways in series {
        for (way, _) in *ways {
            for &(lon, lat) in way {
                min_lon = min_lon.min(lon);
                max_lon = max_lon.max(lon);
                min_lat = min_lat.min(lat);
                max_lat = max_lat.max(lat);
            }
        }
    }

    (min_lon, min_lat, max_lon, max_lat)
}
