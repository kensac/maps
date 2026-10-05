//! Cartography: colors, line widths and dash patterns per feature kind and
//! zoom. Widths are in CSS pixels at 256 px tiles and get multiplied by the
//! output scale.

use crate::classify::{flags, Kind};
use clap::ValueEnum;
use tiny_skia::Color;

/// A color as `0xRRGGBB`.
type Hex = u32;

fn rgb(c: Hex) -> Color {
    rgba(c, 1.0)
}

fn rgba(c: Hex, a: f32) -> Color {
    Color::from_rgba8(
        (c >> 16) as u8,
        (c >> 8) as u8,
        c as u8,
        (a * 255.0).round() as u8,
    )
}

/// Mixes `a` toward `b` by `t`.
fn mix(a: Hex, b: Hex, t: f32) -> Hex {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ThemeName {
    #[default]
    Light,
    Dark,
}

/// Every color the renderer uses.
pub struct Theme {
    pub land: Hex,
    pub water: Hex,
    pub outside: Option<Hex>,
    residential: Hex,
    commercial: Hex,
    retail: Hex,
    industrial: Hex,
    construction: Hex,
    military: Hex,
    garages: Hex,
    religious: Hex,
    cemetery: Hex,
    farmland: Hex,
    orchard: Hex,
    allotments: Hex,
    grass: Hex,
    park: Hex,
    forest: Hex,
    scrub: Hex,
    heath: Hex,
    wetland: Hex,
    beach: Hex,
    sand: Hex,
    rock: Hex,
    glacier: Hex,
    golf: Hex,
    pitch: Hex,
    playground: Hex,
    sports: Hex,
    institution: Hex,
    parking: Hex,
    aerodrome: Hex,
    apron: Hex,
    runway: Hex,
    pedestrian: Hex,
    pier: Hex,
    bridge: Hex,
    platform: Hex,
    building: Hex,
    building_outline: Hex,
    /// Extruded buildings: roof color and the wall color in full light.
    roof: Hex,
    facade: Hex,
    shadow: (Hex, f32),
    tree: (Hex, f32),
    /// `(fill, casing)` per road class, major to minor.
    motorway: (Hex, Hex),
    trunk: (Hex, Hex),
    primary: (Hex, Hex),
    secondary: (Hex, Hex),
    tertiary: (Hex, Hex),
    minor: (Hex, Hex),
    bridge_casing: Hex,
    footway: Hex,
    cycleway: Hex,
    track: Hex,
    rail: Hex,
    rail_dash: Hex,
    tram: Hex,
    fence: Hex,
    wall: Hex,
    hedge: Hex,
    power: Hex,
    aerialway: Hex,
    ferry: Hex,
    boundary: Hex,
}

/// "Daylight": warm paper, soft teal water, one sage family for greenery,
/// white streets ranked by width and casing, and a single amber accent for
/// highways. Land use is a whisper of tint rather than a patchwork.
pub const LIGHT: Theme = Theme {
    land: 0xf5f2eb,
    water: 0x9ccbe0,
    outside: Some(0xe9e5dc),
    residential: 0xf1ede5,
    commercial: 0xf2ebe6,
    retail: 0xf4e9e4,
    industrial: 0xece9ee,
    construction: 0xebe6da,
    military: 0xf0e6e3,
    garages: 0xeceae4,
    religious: 0xefeae2,
    cemetery: 0xd2e2c8,
    farmland: 0xf0efdf,
    orchard: 0xd6e8c6,
    allotments: 0xdbe8cc,
    grass: 0xdcedcc,
    park: 0xd1e9c1,
    forest: 0xbcdba9,
    scrub: 0xd2e3bf,
    heath: 0xdfe4c3,
    wetland: 0xd0e5df,
    beach: 0xf5ebcd,
    sand: 0xf1e7cc,
    rock: 0xe7e2db,
    glacier: 0xeaf3f6,
    golf: 0xd6ecc4,
    pitch: 0xc3e4c6,
    playground: 0xdaefd4,
    sports: 0xd5ecd2,
    institution: 0xf3eedf,
    parking: 0xedebe6,
    aerodrome: 0xedeae4,
    apron: 0xe4e2e7,
    runway: 0xd3d2da,
    pedestrian: 0xf8f6f1,
    pier: 0xf5f2eb,
    bridge: 0xe2ddd5,
    platform: 0xe0dcd6,
    building: 0xe7e0d7,
    building_outline: 0xd5cabe,
    roof: 0xfbf9f5,
    facade: 0xddd2c6,
    shadow: (0x5a4a3a, 0.12),
    tree: (0x86b46e, 0.42),
    motorway: (0xf6c76e, 0xd69a3c),
    trunk: (0xf8d38a, 0xd9a957),
    primary: (0xfff3d4, 0xd6c19a),
    secondary: (0xffffff, 0xd2c8b9),
    tertiary: (0xffffff, 0xd9d1c5),
    minor: (0xffffff, 0xe1dace),
    bridge_casing: 0xa8998a,
    footway: 0xd3ad9e,
    cycleway: 0x86abd8,
    track: 0xbca283,
    rail: 0xaaa5a0,
    rail_dash: 0xffffff,
    tram: 0xaaa5a0,
    fence: 0xcfc8be,
    wall: 0xc4bcb2,
    hedge: 0xb7d4a3,
    power: 0xc2bcb4,
    aerialway: 0x8f8a84,
    ferry: 0x6aa6c4,
    boundary: 0xb38bb0,
};

/// "Midnight": blue-black land, deep ink water, streets that glow warmer
/// and brighter with importance, and slate buildings catching cool light.
pub const DARK: Theme = Theme {
    land: 0x1a1e26,
    water: 0x0d1926,
    outside: Some(0x12151b),
    residential: 0x1c2029,
    commercial: 0x1f222c,
    retail: 0x20222c,
    industrial: 0x1e212c,
    construction: 0x1f2126,
    military: 0x221f25,
    garages: 0x1e2128,
    religious: 0x1e2129,
    cemetery: 0x18251f,
    farmland: 0x1c2224,
    orchard: 0x182720,
    allotments: 0x19261f,
    grass: 0x1b2f25,
    park: 0x1a3326,
    forest: 0x172e22,
    scrub: 0x19251f,
    heath: 0x1d2420,
    wetland: 0x162427,
    beach: 0x26261f,
    sand: 0x242420,
    rock: 0x22242a,
    glacier: 0x1f2a36,
    golf: 0x182a20,
    pitch: 0x16302a,
    playground: 0x1a2c24,
    sports: 0x192b24,
    institution: 0x22232a,
    parking: 0x20242c,
    aerodrome: 0x1e2129,
    apron: 0x252932,
    runway: 0x343a46,
    pedestrian: 0x272c36,
    pier: 0x222731,
    bridge: 0x2a303b,
    platform: 0x2e343f,
    building: 0x2a2f3a,
    building_outline: 0x20242d,
    roof: 0x3b4352,
    facade: 0x272d39,
    shadow: (0x000000, 0.35),
    tree: (0x3d8a68, 0.40),
    motorway: (0xf0a65e, 0x1a1e26),
    trunk: (0xd18f55, 0x1a1e26),
    primary: (0xa3845c, 0x1a1e26),
    secondary: (0x5d5a55, 0x1a1e26),
    tertiary: (0x434852, 0x1a1e26),
    minor: (0x353b46, 0x1a1e26),
    bridge_casing: 0x0b0d11,
    footway: 0x4d5464,
    cycleway: 0x4f6fa8,
    track: 0x51493e,
    rail: 0x4a5160,
    rail_dash: 0x1a1e26,
    tram: 0x4a5160,
    fence: 0x30353f,
    wall: 0x363b46,
    hedge: 0x22392e,
    power: 0x363b46,
    aerialway: 0x6b7280,
    ferry: 0x3c6aa0,
    boundary: 0x8c74b0,
};

impl ThemeName {
    pub fn theme(self) -> &'static Theme {
        match self {
            ThemeName::Light => &LIGHT,
            ThemeName::Dark => &DARK,
        }
    }
}

/// Everything zoom-dependent a style decision needs.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    pub zoom: f32,
    /// Output pixel ratio (2 for retina tiles).
    pub scale: f32,
    /// Output pixels per ground meter.
    pub ppm: f32,
}

#[derive(Clone, Debug)]
pub struct StrokeSpec {
    pub color: Color,
    pub width: f32,
    /// `[on, off]` lengths in pixels.
    pub dash: Option<[f32; 2]>,
    pub round: bool,
}

/// How to draw a line feature: an optional casing underneath (drawn for the
/// whole layer before any fills, so junctions merge cleanly), then the line.
#[derive(Clone, Debug, Default)]
pub struct LineStyle {
    pub casing: Option<StrokeSpec>,
    pub line: Option<StrokeSpec>,
}

#[derive(Clone, Debug)]
pub struct AreaStyle {
    pub fill: Color,
    pub outline: Option<StrokeSpec>,
}

/// Interpolates `(zoom, width)` stops exponentially (widths double per zoom
/// level beyond the last stop, like ground distances do).
fn stops(z: f32, s: &[(f32, f32)]) -> f32 {
    let (z0, w0) = s[0];
    if z <= z0 {
        return w0;
    }
    for pair in s.windows(2) {
        let ((za, wa), (zb, wb)) = (pair[0], pair[1]);
        if z <= zb {
            let t = (z - za) / (zb - za);
            return (wa.ln() + (wb.ln() - wa.ln()) * t).exp();
        }
    }
    let (zl, wl) = s[s.len() - 1];
    wl * (z - zl).exp2()
}

/// Road fill width in pixels for a road class.
fn road_width(kind: Kind, z: f32) -> f32 {
    use Kind::*;
    let s: &[(f32, f32)] = match kind {
        Motorway => &[
            (5.0, 0.5),
            (10.0, 1.6),
            (13.0, 4.0),
            (15.0, 8.0),
            (18.0, 28.0),
        ],
        Trunk => &[
            (6.0, 0.5),
            (10.0, 1.4),
            (13.0, 3.6),
            (15.0, 7.5),
            (18.0, 26.0),
        ],
        Primary => &[
            (8.0, 0.5),
            (11.0, 1.4),
            (13.0, 3.4),
            (15.0, 7.0),
            (18.0, 24.0),
        ],
        Secondary => &[
            (9.0, 0.5),
            (12.0, 1.5),
            (13.0, 2.8),
            (15.0, 6.0),
            (18.0, 21.0),
        ],
        Tertiary => &[
            (10.0, 0.5),
            (12.0, 1.1),
            (14.0, 3.4),
            (15.0, 5.0),
            (18.0, 18.0),
        ],
        Minor | PedestrianStreet => &[(12.0, 0.5), (14.0, 2.0), (15.0, 3.6), (18.0, 15.0)],
        Service => &[(14.0, 0.6), (15.0, 1.6), (16.0, 2.8), (18.0, 8.0)],
        Track => &[(14.0, 0.6), (16.0, 1.3), (18.0, 3.0)],
        Footway | Cycleway => &[(15.0, 0.7), (16.0, 1.0), (18.0, 2.0), (20.0, 4.0)],
        _ => &[(0.0, 1.0)],
    };
    stops(z, s)
}

fn solid(color: Color, width: f32) -> Option<StrokeSpec> {
    Some(StrokeSpec {
        color,
        width,
        dash: None,
        round: true,
    })
}

fn dashed(color: Color, width: f32, on: f32, off: f32) -> Option<StrokeSpec> {
    Some(StrokeSpec {
        color,
        width,
        dash: Some([on, off]),
        round: false,
    })
}

impl Theme {
    pub fn area(&self, kind: Kind, c: &Ctx) -> Option<AreaStyle> {
        use Kind::*;
        let fill = match kind {
            Land => self.land,
            Water => self.water,
            Glacier => self.glacier,
            Wetland => self.wetland,
            Beach => self.beach,
            Sand => self.sand,
            BareRock => self.rock,
            Forest => self.forest,
            Scrub => self.scrub,
            Heath => self.heath,
            Grass => self.grass,
            Park => self.park,
            Golf => self.golf,
            Pitch => self.pitch,
            Playground => self.playground,
            Sports => self.sports,
            Cemetery => self.cemetery,
            Farmland => self.farmland,
            Orchard => self.orchard,
            Allotments => self.allotments,
            Residential => self.residential,
            Commercial => self.commercial,
            Retail => self.retail,
            Industrial | RailwayLand => self.industrial,
            Construction => self.construction,
            Military => self.military,
            Garages => self.garages,
            Religious => self.religious,
            School | Hospital => self.institution,
            Parking => self.parking,
            Aerodrome => self.aerodrome,
            Apron => self.apron,
            PedestrianArea => self.pedestrian,
            Pier => self.pier,
            BridgeArea => self.bridge,
            Platform => self.platform,
            RunwayArea => self.runway,
            Building => self.building,
            _ => return None,
        };
        let s = c.scale;
        let outline = match kind {
            Building if c.zoom >= 15.0 => solid(rgb(self.building_outline), 0.6 * s),
            Pitch | Parking if c.zoom >= 16.0 => solid(rgb(mix(fill, 0x000000, 0.12)), 0.6 * s),
            Military if c.zoom >= 12.0 => solid(rgba(0xc24e4e, 0.5), 0.8 * s),
            _ => None,
        };
        Some(AreaStyle {
            fill: rgb(fill),
            outline,
        })
    }

    pub fn line(&self, kind: Kind, f: u8, height: f32, c: &Ctx) -> LineStyle {
        use Kind::*;
        let (z, s) = (c.zoom, c.scale);
        let meters = |m: f32, min: f32| (m * c.ppm).max(min * s);
        let tunnel = f & flags::TUNNEL != 0;
        let bridge = f & flags::BRIDGE != 0;
        let mut width_mul = 1.0;
        if f & flags::LINK != 0 {
            width_mul *= 0.7;
        }
        if f & flags::MINOR != 0 {
            width_mul *= 0.6;
        }

        let road = |fill: Hex, casing: Hex, casing_from: f32| {
            let w = road_width(kind, z) * width_mul * s;
            let outline = (w * 0.12).clamp(0.6 * s, 2.0 * s);
            let fill = if tunnel {
                mix(fill, self.land, 0.55)
            } else {
                fill
            };
            let casing_color = if bridge { self.bridge_casing } else { casing };
            let casing = (z >= casing_from).then(|| StrokeSpec {
                color: rgb(casing_color),
                width: w + 2.0 * outline,
                dash: tunnel.then_some([w.max(3.0 * s), (w * 0.6).max(2.0 * s)]),
                // Round caps would leave blobs where bridges and tunnels end.
                round: !tunnel && !bridge,
            });
            LineStyle {
                casing,
                line: solid(rgb(fill), w),
            }
        };
        let path = |color: Hex| {
            let w = road_width(kind, z) * width_mul * s;
            let a = if tunnel { 0.45 } else { 1.0 };
            LineStyle {
                casing: (z >= 16.0).then(|| StrokeSpec {
                    color: rgba(self.land, 0.6),
                    width: w + 2.0 * s,
                    dash: None,
                    round: true,
                }),
                line: dashed(
                    rgba(color, a),
                    w,
                    (w * 2.5).max(2.0 * s),
                    (w * 1.8).max(1.5 * s),
                ),
            }
        };
        let rail = |w_stops: &[(f32, f32)], dashes_from: f32| {
            let w = stops(z, w_stops) * width_mul * s;
            if tunnel {
                return LineStyle {
                    casing: None,
                    line: dashed(rgba(self.rail, 0.45), w * 0.7, 4.0 * s, 3.0 * s),
                };
            }
            let casing = solid(rgb(self.rail), w).map(|mut st| {
                st.round = false;
                st
            });
            let line = (z >= dashes_from)
                .then(|| dashed(rgb(self.rail_dash), w * 0.45, w * 3.0, w * 3.0))
                .flatten();
            LineStyle { casing, line }
        };
        let water = |w_stops: &[(f32, f32)]| LineStyle {
            casing: None,
            line: solid(rgb(self.water), stops(z, w_stops) * s),
        };

        match kind {
            Motorway => road(self.motorway.0, self.motorway.1, 9.0),
            Trunk => road(self.trunk.0, self.trunk.1, 10.0),
            Primary => road(self.primary.0, self.primary.1, 11.0),
            Secondary => road(self.secondary.0, self.secondary.1, 12.0),
            Tertiary => road(self.tertiary.0, self.tertiary.1, 13.0),
            Minor | Service => road(self.minor.0, self.minor.1, 14.0),
            PedestrianStreet => road(self.pedestrian, self.minor.1, 14.0),
            Track => path(self.track),
            Footway => path(self.footway),
            Cycleway => path(self.cycleway),
            Rail => rail(&[(8.0, 0.6), (12.0, 1.4), (15.0, 2.6), (18.0, 4.0)], 13.0),
            Subway | LightRail => rail(&[(11.0, 0.6), (14.0, 1.4), (18.0, 3.0)], 15.0),
            Tram => LineStyle {
                casing: None,
                line: solid(
                    rgba(self.tram, if tunnel { 0.4 } else { 1.0 }),
                    stops(z, &[(13.0, 0.6), (16.0, 1.2), (18.0, 2.0)]) * s,
                ),
            },
            River => water(&[(8.0, 0.8), (12.0, 2.0), (14.0, 4.5), (18.0, 16.0)]),
            Canal => water(&[(10.0, 0.8), (14.0, 3.5), (18.0, 12.0)]),
            Stream => water(&[(13.0, 0.6), (15.0, 1.4), (18.0, 4.0)]),
            Ditch => water(&[(15.0, 0.5), (18.0, 2.0)]),
            Runway => LineStyle {
                casing: None,
                line: solid(rgb(self.runway), meters(45.0, 1.5)).map(|mut st| {
                    st.round = false;
                    st
                }),
            },
            Taxiway => LineStyle {
                casing: None,
                line: solid(rgb(self.runway), meters(18.0, 0.7)),
            },
            PierLine => LineStyle {
                casing: None,
                line: solid(rgb(self.pier), meters(5.0, 1.0)),
            },
            Breakwater => LineStyle {
                casing: None,
                line: solid(rgb(self.rock), meters(6.0, 1.0)),
            },
            Fence => LineStyle {
                casing: None,
                line: solid(rgb(self.fence), 0.6 * s),
            },
            Wall => LineStyle {
                casing: None,
                line: solid(rgb(self.wall), 1.0 * s),
            },
            Hedge => LineStyle {
                casing: None,
                line: solid(rgb(self.hedge), meters(2.0, 1.0)),
            },
            PowerLine => LineStyle {
                casing: None,
                line: solid(rgba(self.power, 0.7), 0.5 * s),
            },
            Aerialway => LineStyle {
                casing: solid(rgb(self.aerialway), 1.0 * s),
                line: dashed(rgb(self.aerialway), 4.0 * s, 1.0 * s, 12.0 * s),
            },
            Ferry => LineStyle {
                casing: None,
                line: dashed(rgba(self.ferry, 0.45), 0.8 * s, 5.0 * s, 5.0 * s),
            },
            Boundary => {
                let level = height as u8;
                let w = match level {
                    0..=4 => 2.0,
                    5..=6 => 1.5,
                    _ => 1.0,
                } * s;
                LineStyle {
                    casing: (z >= 11.0).then(|| StrokeSpec {
                        color: rgba(self.boundary, 0.14),
                        width: w * 5.0,
                        dash: None,
                        round: true,
                    }),
                    line: dashed(rgba(self.boundary, 0.75), w, 6.0 * s, 3.0 * s),
                }
            }
            _ => LineStyle::default(),
        }
    }

    /// Tree canopy: fill color and radius in pixels.
    pub fn tree(&self, c: &Ctx) -> (Color, f32) {
        (
            rgba(self.tree.0, self.tree.1),
            (3.5 * c.ppm).max(1.2 * c.scale),
        )
    }

    pub fn roof(&self) -> Color {
        rgb(self.roof)
    }

    /// Wall color for a facade receiving `light` ∈ [0, 1].
    pub fn facade(&self, light: f32) -> Color {
        rgb(mix(mix(self.facade, 0x000000, 0.22), self.facade, light))
    }

    pub fn building_outline(&self) -> Color {
        rgb(self.building_outline)
    }

    pub fn shadow(&self) -> Color {
        rgba(self.shadow.0, self.shadow.1)
    }

    pub fn land_color(&self) -> Color {
        rgb(self.land)
    }

    pub fn water_color(&self) -> Color {
        rgb(self.water)
    }

    pub fn outside_color(&self) -> Option<Color> {
        self.outside.map(rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_interpolate_exponentially() {
        let s = [(10.0, 1.0), (12.0, 4.0)];
        assert_eq!(stops(9.0, &s), 1.0);
        assert!((stops(11.0, &s) - 2.0).abs() < 1e-5);
        assert!((stops(13.0, &s) - 8.0).abs() < 1e-5);
    }

    #[test]
    fn major_roads_are_wider() {
        for z in [10.0, 14.0, 18.0] {
            assert!(road_width(Kind::Motorway, z) > road_width(Kind::Primary, z));
            assert!(road_width(Kind::Primary, z) > road_width(Kind::Minor, z));
        }
    }

    #[test]
    fn every_area_kind_has_a_fill() {
        let c = Ctx {
            zoom: 16.0,
            scale: 1.0,
            ppm: 1.0,
        };
        for k in [
            Kind::Land,
            Kind::Water,
            Kind::Park,
            Kind::Building,
            Kind::RunwayArea,
        ] {
            assert!(LIGHT.area(k, &c).is_some() && DARK.area(k, &c).is_some());
        }
    }
}
