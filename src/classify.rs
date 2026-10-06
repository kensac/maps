//! Turns raw OSM tags into a compact, renderable taxonomy of physical things,
//! with their real (or typical) dimensions.
//!
//! The renderer only draws what physically exists on the ground: names,
//! addresses, routes and other abstract data are ignored.

/// The OSM keys the renderer cares about, pulled out of a tag list in one pass.
#[derive(Default, Debug, Clone, Copy)]
pub struct Tags<'a> {
    pub highway: Option<&'a str>,
    pub railway: Option<&'a str>,
    pub waterway: Option<&'a str>,
    pub building: Option<&'a str>,
    pub building_part: Option<&'a str>,
    pub landuse: Option<&'a str>,
    pub leisure: Option<&'a str>,
    pub natural: Option<&'a str>,
    pub amenity: Option<&'a str>,
    pub aeroway: Option<&'a str>,
    pub man_made: Option<&'a str>,
    pub barrier: Option<&'a str>,
    pub power: Option<&'a str>,
    pub boundary: Option<&'a str>,
    pub admin_level: Option<&'a str>,
    pub route: Option<&'a str>,
    pub aerialway: Option<&'a str>,
    pub public_transport: Option<&'a str>,
    pub place: Option<&'a str>,
    pub area: Option<&'a str>,
    pub area_highway: Option<&'a str>,
    pub layer: Option<&'a str>,
    pub bridge: Option<&'a str>,
    pub tunnel: Option<&'a str>,
    pub covered: Option<&'a str>,
    pub location: Option<&'a str>,
    pub service: Option<&'a str>,
    pub footway: Option<&'a str>,
    pub parking: Option<&'a str>,
    pub height: Option<&'a str>,
    pub levels: Option<&'a str>,
    pub min_height: Option<&'a str>,
    pub min_level: Option<&'a str>,
    pub sport: Option<&'a str>,
    pub golf: Option<&'a str>,
    pub emergency: Option<&'a str>,
    pub historic: Option<&'a str>,
    pub tourism: Option<&'a str>,
    pub playground: Option<&'a str>,
    pub leaf_type: Option<&'a str>,
    pub wetland: Option<&'a str>,
    pub shelter: Option<&'a str>,
    pub roof_shape: Option<&'a str>,
    pub roof_height: Option<&'a str>,
    pub roof_levels: Option<&'a str>,
    pub roof_colour: Option<&'a str>,
    pub roof_orientation: Option<&'a str>,
    pub building_colour: Option<&'a str>,
    pub tower_type: Option<&'a str>,
    pub tower_construction: Option<&'a str>,
    pub kind: Option<&'a str>,
}

impl<'a> Tags<'a> {
    pub fn parse(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Self {
        let mut t = Tags::default();
        for (k, v) in tags {
            let slot = match k {
                "highway" => &mut t.highway,
                "railway" => &mut t.railway,
                "waterway" => &mut t.waterway,
                "building" => &mut t.building,
                "building:part" => &mut t.building_part,
                "landuse" => &mut t.landuse,
                "leisure" => &mut t.leisure,
                "natural" => &mut t.natural,
                "amenity" => &mut t.amenity,
                "aeroway" => &mut t.aeroway,
                "man_made" => &mut t.man_made,
                "barrier" => &mut t.barrier,
                "power" => &mut t.power,
                "boundary" => &mut t.boundary,
                "admin_level" => &mut t.admin_level,
                "route" => &mut t.route,
                "aerialway" => &mut t.aerialway,
                "public_transport" => &mut t.public_transport,
                "place" => &mut t.place,
                "area" => &mut t.area,
                "area:highway" => &mut t.area_highway,
                "layer" => &mut t.layer,
                "bridge" => &mut t.bridge,
                "tunnel" => &mut t.tunnel,
                "covered" => &mut t.covered,
                "location" => &mut t.location,
                "service" => &mut t.service,
                "footway" => &mut t.footway,
                "parking" => &mut t.parking,
                "height" | "building:height" => &mut t.height,
                "building:levels" => &mut t.levels,
                "min_height" | "building:min_height" => &mut t.min_height,
                "building:min_level" => &mut t.min_level,
                "sport" => &mut t.sport,
                "golf" => &mut t.golf,
                "emergency" => &mut t.emergency,
                "historic" => &mut t.historic,
                "tourism" => &mut t.tourism,
                "playground" => &mut t.playground,
                "leaf_type" => &mut t.leaf_type,
                "wetland" => &mut t.wetland,
                "shelter" => &mut t.shelter,
                "roof:shape" => &mut t.roof_shape,
                "roof:height" => &mut t.roof_height,
                "roof:levels" => &mut t.roof_levels,
                "roof:colour" | "roof:color" => &mut t.roof_colour,
                "roof:orientation" => &mut t.roof_orientation,
                "building:colour" | "building:color" | "colour" => &mut t.building_colour,
                "tower:type" => &mut t.tower_type,
                "tower:construction" => &mut t.tower_construction,
                "type" => &mut t.kind,
                _ => continue,
            };
            *slot = Some(v);
        }
        t
    }

    fn yes(v: Option<&str>) -> bool {
        matches!(v, Some(v) if v != "no" && v != "false" && v != "0")
    }

    pub fn layer(&self) -> i8 {
        self.layer
            .and_then(|l| l.trim().parse::<i8>().ok())
            .unwrap_or(0)
            .clamp(-5, 5)
    }

    pub fn flags(&self) -> u8 {
        let mut f = 0;
        if Self::yes(self.bridge) {
            f |= flags::BRIDGE;
        }
        if Self::yes(self.tunnel) || Self::yes(self.covered) {
            f |= flags::TUNNEL;
        }
        if matches!(self.highway, Some(h) if h.ends_with("_link")) {
            f |= flags::LINK;
        }
        if matches!(
            self.service,
            Some("parking_aisle" | "driveway" | "siding" | "yard" | "spur" | "crossover")
        ) || matches!(self.footway, Some("sidewalk" | "crossing"))
            || self.power == Some("minor_line")
        {
            f |= flags::MINOR;
        }
        if matches!(self.location, Some("roof" | "rooftop")) {
            f |= flags::ON_ROOF;
        }
        if self.leaf_type == Some("needleleaved") {
            f |= flags::CONIFER;
        }
        if Self::yes(self.building_part) {
            f |= flags::PART;
        }
        f
    }

    /// A length in meters from a tag like `12.5`, `30 m` or `100'`.
    fn meters(v: &str) -> Option<f32> {
        let num: String = v
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let n = num.parse::<f32>().ok()?;
        let feet = v.contains('\'') || v.contains("ft");
        Some(if feet { n * 0.3048 } else { n }).filter(|m| m.is_finite() && *m >= 0.0)
    }

    /// Tagged height in meters, from `height` or `building:levels`.
    pub fn height(&self) -> Option<f32> {
        self.height.and_then(Self::meters).or_else(|| {
            self.levels
                .and_then(|l| l.trim().parse::<f32>().ok())
                .map(|l| l * 3.2 + 1.0)
        })
    }

    /// Architectural detail of a solid: roof shape and colors.
    pub fn detail(&self) -> Detail {
        let roof = match self.roof_shape {
            Some("gabled" | "round" | "saltbox" | "double_saltbox" | "quadruple_saltbox") => {
                RoofShape::Gabled
            }
            Some("hipped" | "half-hipped" | "mansard" | "gambrel" | "side_hipped") => {
                RoofShape::Hipped
            }
            Some("pyramidal" | "cone" | "pyramid") => RoofShape::Pyramidal,
            Some("skillion" | "lean_to") => RoofShape::Skillion,
            Some("dome" | "onion") => RoofShape::Dome,
            _ => RoofShape::Flat,
        };
        let roof_height = self
            .roof_height
            .and_then(Self::meters)
            .or_else(|| {
                self.roof_levels
                    .and_then(|l| l.trim().parse::<f32>().ok())
                    .map(|l| l * 3.0)
            })
            .unwrap_or(0.0);
        let levels = self
            .levels
            .and_then(|l| l.trim().parse::<f32>().ok())
            .unwrap_or(0.0);
        Detail {
            roof,
            roof_height,
            across: self.roof_orientation == Some("across"),
            roof_colour: self.roof_colour.and_then(parse_colour),
            facade_colour: self.building_colour.and_then(parse_colour),
            levels,
        }
    }

    /// Height of the bottom of a structure above the ground, in meters.
    pub fn min_height(&self) -> f32 {
        self.min_height
            .and_then(Self::meters)
            .or_else(|| {
                self.min_level
                    .and_then(|l| l.trim().parse::<f32>().ok())
                    .map(|l| l * 3.2)
            })
            .unwrap_or(0.0)
    }
}

/// What kind of tower a `man_made=tower` is, for its model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum TowerType {
    #[default]
    Generic,
    /// A single tapered pole with antenna panels.
    Monopole,
    /// A braced steel lattice.
    Lattice,
    /// A lattice held by guy wires.
    Guyed,
    /// A pole carrying floodlights.
    Lighting,
    /// A masonry tower topped by a spire (bell and clock towers).
    Belfry,
    /// A slender round tower with a balcony and a pointed cap.
    Minaret,
    /// A tower with a viewing deck.
    Observation,
}

impl Tags<'_> {
    /// Model variant of a point object (currently: tower types).
    pub fn variant(&self) -> u8 {
        if self.man_made != Some("tower") {
            return 0;
        }
        let t = match (self.tower_type, self.tower_construction) {
            (_, Some("guyed_lattice" | "guyed")) => TowerType::Guyed,
            (_, Some("lattice" | "truss")) => TowerType::Lattice,
            (Some("lighting"), _) => TowerType::Lighting,
            (Some("bell_tower" | "clock_tower" | "church"), _) => TowerType::Belfry,
            (Some("minaret"), _) => TowerType::Minaret,
            (Some("observation"), _) => TowerType::Observation,
            (Some("communication" | "telecommunication" | "radar"), _) => TowerType::Monopole,
            (_, Some("monopole" | "mast" | "concealed" | "tube" | "freestanding")) => {
                TowerType::Monopole
            }
            _ => TowerType::Generic,
        };
        t as u8
    }
}

impl TowerType {
    pub fn from_u8(v: u8) -> Self {
        use TowerType::*;
        [
            Generic,
            Monopole,
            Lattice,
            Guyed,
            Lighting,
            Belfry,
            Minaret,
            Observation,
        ]
        .get(v as usize)
        .copied()
        .unwrap_or(Generic)
    }

    /// Typical height in meters.
    fn height(self) -> f32 {
        match self {
            TowerType::Lighting => 20.0,
            TowerType::Monopole => 30.0,
            TowerType::Lattice | TowerType::Guyed => 45.0,
            TowerType::Belfry => 30.0,
            TowerType::Minaret => 35.0,
            TowerType::Observation | TowerType::Generic => 25.0,
        }
    }
}

/// Shape of a roof.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoofShape {
    #[default]
    Flat,
    /// Two slopes meeting at a ridge along the building.
    Gabled,
    /// Four slopes: a shorter ridge, sloped ends.
    Hipped,
    /// Slopes meeting at a point (also cones).
    Pyramidal,
    /// A single slope.
    Skillion,
    /// A dome (also onion domes).
    Dome,
}

/// Architectural detail of an extruded solid.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Detail {
    pub roof: RoofShape,
    /// Height of the roof itself in meters; 0 picks one from the footprint.
    pub roof_height: f32,
    /// Ridge across the building rather than along it.
    pub across: bool,
    /// Mapped colors as `0xRRGGBB`.
    pub roof_colour: Option<u32>,
    pub facade_colour: Option<u32>,
    /// Number of floors above ground, if tagged.
    pub levels: f32,
}

/// Parses an OSM colour: `#rrggbb`, `#rgb` or a common color name.
pub fn parse_colour(v: &str) -> Option<u32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        return match hex.len() {
            6 => u32::from_str_radix(hex, 16).ok(),
            3 => {
                let n = u32::from_str_radix(hex, 16).ok()?;
                let (r, g, b) = ((n >> 8) & 0xf, (n >> 4) & 0xf, n & 0xf);
                Some(((r * 17) << 16) | ((g * 17) << 8) | (b * 17))
            }
            _ => None,
        };
    }
    Some(match v.as_str() {
        "white" => 0xf4f2ee,
        "black" => 0x2b2b2b,
        "gray" | "grey" => 0x8f8f8f,
        "darkgray" | "darkgrey" | "dark_grey" | "dark_gray" => 0x5f5f5f,
        "lightgray" | "lightgrey" | "light_grey" | "light_gray" | "silver" => 0xc4c4c4,
        "red" => 0xb5483a,
        "darkred" | "maroon" => 0x7a2e26,
        "brown" => 0x8a5a3c,
        "tan" | "beige" => 0xd8c7a6,
        "cream" | "ivory" => 0xefe6cf,
        "yellow" => 0xe4c75a,
        "gold" => 0xc9a640,
        "orange" => 0xd98a45,
        "green" => 0x5f8a55,
        "darkgreen" => 0x3f5f3a,
        "blue" => 0x4f6f9f,
        "lightblue" => 0x9fbfd8,
        "navy" | "darkblue" => 0x2f3f6a,
        "pink" => 0xd8a0a0,
        "copper" | "teal" => 0x5f9f8f,
        "terracotta" => 0xb86a4a,
        _ => return None,
    })
}

/// Bit flags carried alongside a feature's [`Kind`].
pub mod flags {
    pub const BRIDGE: u8 = 1;
    pub const TUNNEL: u8 = 2;
    pub const LINK: u8 = 4;
    pub const MINOR: u8 = 8;
    /// Stands on a roof: its base is lifted to the building beneath.
    pub const ON_ROOF: u8 = 16;
    /// Needle-leaved tree.
    pub const CONIFER: u8 = 32;
    /// A `building:part`; the outline it belongs to is not drawn.
    pub const PART: u8 = 64;
}

/// What a feature is. Ordering within a group matters: it is the draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Kind {
    // Ground areas.
    Land,
    Water,
    Pool,
    Glacier,
    Wetland,
    Tidalflat,
    Reef,
    Beach,
    Sand,
    BareRock,
    Forest,
    Scrub,
    Heath,
    Grass,
    Park,
    Golf,
    GolfRough,
    GolfFairway,
    GolfGreen,
    GolfTee,
    GolfBunker,
    Pitch,
    Tennis,
    HardCourt,
    Baseball,
    Playground,
    Sports,
    Cemetery,
    Farmland,
    Orchard,
    Allotments,
    Residential,
    Commercial,
    Retail,
    Industrial,
    RailwayLand,
    Construction,
    Military,
    Garages,
    Religious,
    School,
    Hospital,
    Attraction,
    Parking,
    ParkingSpace,
    Fuel,
    OutdoorSeating,
    Planter,
    Aerodrome,
    Apron,
    PedestrianArea,
    RoadArea,
    TrafficIsland,
    Pier,
    BridgeArea,
    Platform,
    RunwayArea,
    DamArea,
    // Extruded solids.
    Building,
    Tank,
    Canopy,
    Bleachers,
    Tomb,
    // Lines.
    Ditch,
    Stream,
    Canal,
    River,
    Runway,
    Taxiway,
    ApronLine,
    PierLine,
    Breakwater,
    Kerb,
    Embankment,
    Footway,
    Cycleway,
    Track,
    Service,
    PedestrianStreet,
    Minor,
    Tertiary,
    Secondary,
    Primary,
    Trunk,
    Motorway,
    Tram,
    LightRail,
    Subway,
    Rail,
    Hedge,
    Fence,
    Wall,
    LowBarrier,
    Cliff,
    Dam,
    Weir,
    TreeRow,
    Pipeline,
    Gantry,
    JetBridge,
    PowerLine,
    Aerialway,
    Ferry,
    Boundary,
    // Point objects.
    Tree,
    Shrub,
    Stone,
    StreetLamp,
    TrafficSignal,
    StopSign,
    PowerPole,
    PowerTower,
    Flagpole,
    Mast,
    Chimney,
    Tower,
    WaterTower,
    Crane,
    Bollard,
    Block,
    Hydrant,
    Bench,
    PicnicTable,
    WasteBasket,
    PostBox,
    BicycleParking,
    DrinkingWater,
    Phone,
    SubwayEntrance,
    Shelter,
    Monument,
    Artwork,
    PlayEquipment,
    RailSignal,
    BufferStop,
    Cabinet,
    Windsock,
    NavLight,
}

/// Coarse draw layers, rendered bottom to top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Group {
    Land,
    Areas,
    AreaOverlays,
    Waterways,
    GroundLines,
    /// Tunnels and anything below ground.
    TransportTunnels,
    /// Buildings, solids and point objects. Flat views draw them here; 3D
    /// views paint them back to front after everything on the ground.
    Objects,
    /// Ground-level roads and rails.
    TransportGround,
    /// Bridges and elevated lines.
    TransportBridges,
    Overlays,
}

impl Group {
    pub fn is_transport(self) -> bool {
        matches!(
            self,
            Group::TransportTunnels | Group::TransportGround | Group::TransportBridges
        )
    }
}

impl Kind {
    /// Polygon geometry: ground areas and extruded solids.
    pub fn is_area(self) -> bool {
        self <= Kind::Tomb
    }

    /// Solids extruded from their footprint.
    pub fn is_extrusion(self) -> bool {
        (Kind::Building..=Kind::Tomb).contains(&self)
    }

    pub fn is_point(self) -> bool {
        self >= Kind::Tree
    }

    pub fn is_transport(self) -> bool {
        (Kind::Footway..=Kind::Rail).contains(&self)
    }

    pub fn group(self, layer: i8, f: u8) -> Group {
        use Kind::*;
        match self {
            Land => Group::Land,
            Pier | BridgeArea | Platform | RunwayArea | DamArea | RoadArea | TrafficIsland
            | ParkingSpace => Group::AreaOverlays,
            k if k.is_extrusion() || k.is_point() => Group::Objects,
            k if k.is_area() => Group::Areas,
            Ditch | Stream | Canal | River => Group::Waterways,
            Runway | Taxiway | ApronLine | PierLine | Breakwater | Kerb | Embankment => {
                Group::GroundLines
            }
            k if k.is_transport() && (layer < 0 || f & flags::TUNNEL != 0) => {
                Group::TransportTunnels
            }
            // Bridges without a layer tag are still above ground.
            k if k.is_transport() && (layer > 0 || f & flags::BRIDGE != 0) => {
                Group::TransportBridges
            }
            k if k.is_transport() => Group::TransportGround,
            _ => Group::Overlays,
        }
    }

    /// Zoom level below which the feature is never drawn.
    pub fn min_zoom(self, f: u8) -> f32 {
        use Kind::*;
        let minor = f & flags::MINOR != 0;
        match self {
            Footway if minor => 17.0,
            Service if minor => 15.0,
            PowerLine if minor => 16.0,
            Land => 0.0,
            Water => 4.0,
            Glacier | Forest | Wetland => 6.0,
            Farmland | Residential | Industrial | Commercial | Retail | Grass | Park | Scrub
            | Heath | Beach | Sand | BareRock | Aerodrome | Military | RailwayLand | Cemetery
            | Golf | Orchard | Tidalflat | Reef => 8.0,
            Construction | Allotments | Religious | School | Hospital | Garages | Apron
            | Attraction => 10.0,
            Sports | Pitch | Tennis | HardCourt | Baseball | RunwayArea | GolfFairway
            | GolfRough | GolfGreen | GolfTee | GolfBunker => 12.0,
            Playground | Parking | PedestrianArea | Pier | BridgeArea | Platform | Fuel
            | DamArea | Pool => 13.0,
            OutdoorSeating | RoadArea | TrafficIsland => 15.0,
            Planter => 16.0,
            ParkingSpace => 17.0,
            Building => 13.0,
            Tank | Canopy => 14.0,
            Bleachers | Tomb => 16.0,
            River => 8.0,
            Canal => 10.0,
            Stream => 13.0,
            Ditch => 15.0,
            Runway => 10.0,
            Taxiway => 12.0,
            ApronLine => 16.0,
            PierLine | Breakwater => 14.0,
            Embankment => 15.0,
            Kerb => 18.0,
            Motorway => 5.0,
            Trunk => 6.0,
            Primary => 8.0,
            Secondary => 9.0,
            Tertiary => 10.0,
            Minor | PedestrianStreet => 12.0,
            Service | Track => 14.0,
            Footway | Cycleway => 15.0,
            Rail => 8.0,
            Subway | LightRail => 11.0,
            Tram => 13.0,
            Ferry => 9.0,
            Boundary => 4.0,
            Aerialway | PowerLine | Dam => 14.0,
            JetBridge | Gantry | Cliff => 15.0,
            Hedge | Fence | Wall | LowBarrier | Weir | TreeRow | Pipeline => 16.0,
            Tree | PowerTower | Chimney | Tower | WaterTower | Crane | Mast => 16.0,
            Shrub | StreetLamp | TrafficSignal | PowerPole | Flagpole | SubwayEntrance
            | Shelter | Monument | Artwork | Windsock => 17.0,
            Stone | StopSign | Bollard | Block | Hydrant | Bench | PicnicTable | WasteBasket
            | PostBox | BicycleParking | DrinkingWater | Phone | PlayEquipment | RailSignal
            | BufferStop | Cabinet | NavLight => 18.0,
        }
    }

    /// Typical height in meters when a feature carries no height tag.
    pub fn default_height(self, t: &Tags) -> f32 {
        use Kind::*;
        match self {
            Building => building_height(t.building_part.or(t.building).unwrap_or("yes")),
            Tank => match t.man_made {
                Some("silo") => 20.0,
                Some("water_tower") => 25.0,
                Some("chimney") => 40.0,
                _ => 12.0,
            },
            Canopy => 4.5,
            Bleachers => 5.0,
            Tomb => 2.5,
            Hedge => 1.8,
            Fence => 1.5,
            Wall => 2.5,
            LowBarrier => 0.85,
            Cliff => 8.0,
            Dam => 10.0,
            Weir => 1.5,
            TreeRow => 9.0,
            Pipeline => 1.0,
            Gantry => 7.0,
            JetBridge => 4.5,
            PowerLine if t.power == Some("minor_line") => 10.0,
            PowerLine => 22.0,
            Aerialway => 25.0,
            Tree => 9.0,
            Shrub => 1.5,
            Stone => 0.8,
            StreetLamp => 8.0,
            TrafficSignal => 5.5,
            StopSign => 2.4,
            PowerPole => 11.0,
            PowerTower => 35.0,
            Flagpole => 10.0,
            Mast => 30.0,
            Chimney => 40.0,
            Tower => TowerType::from_u8(t.variant()).height(),
            WaterTower => 30.0,
            Crane => 40.0,
            Bollard => 0.9,
            Block => 0.8,
            Hydrant => 0.8,
            Bench => 0.8,
            PicnicTable => 0.75,
            WasteBasket => 1.0,
            PostBox => 1.2,
            BicycleParking => 0.9,
            DrinkingWater => 1.0,
            Phone => 2.2,
            SubwayEntrance => 3.0,
            Shelter => 2.7,
            Monument => 3.0,
            Artwork => 3.0,
            PlayEquipment => 2.5,
            RailSignal => 4.0,
            BufferStop => 1.2,
            Cabinet => 1.4,
            Windsock => 6.0,
            NavLight => 0.3,
            _ => 0.0,
        }
    }
}

/// Typical height of a building by its `building=*` type.
fn building_height(kind: &str) -> f32 {
    match kind {
        "house" | "detached" | "semidetached_house" | "bungalow" | "farm" | "cabin" => 7.0,
        "terrace" | "townhouse" | "terraced" => 9.0,
        "apartments" => 15.0,
        "residential" | "dormitory" => 10.0,
        "garage" | "garages" | "carport" | "kiosk" | "toilets" | "service" => 3.0,
        "shed" | "hut" | "greenhouse" => 2.5,
        "roof" => 4.5,
        "commercial" | "civic" | "public" | "government" => 9.0,
        "retail" | "supermarket" => 6.0,
        "industrial" | "warehouse" | "manufacture" => 9.0,
        "office" => 20.0,
        "hotel" => 25.0,
        "school" | "kindergarten" => 12.0,
        "university" | "college" => 15.0,
        "hospital" => 25.0,
        "church" | "chapel" | "mosque" | "synagogue" | "temple" | "cathedral" => 15.0,
        "parking" => 10.0,
        "stadium" | "grandstand" => 20.0,
        "train_station" | "transportation" => 10.0,
        _ => 6.0,
    }
}

/// Classifies a closed ring (way or multipolygon) as an area kind.
pub fn area_kind(t: &Tags) -> Option<Kind> {
    use Kind::*;
    let underground = matches!(t.location, Some("underground" | "indoor"));
    if underground || t.layer() < 0 && (t.building.is_some() || t.building_part.is_some()) {
        return None;
    }
    if Tags::yes(t.building_part) {
        return Some(Building);
    }
    match t.building {
        Some("roof" | "carport") => return Some(Canopy),
        Some(b) if b != "no" => return Some(Building),
        _ => {}
    }
    if matches!(t.aeroway, Some("terminal" | "hangar")) {
        return Some(Building);
    }
    if let Some(m) = t.man_made {
        match m {
            "storage_tank" | "silo" | "water_tower" | "chimney" => return Some(Tank),
            "tower" => return Some(Building),
            "pier" | "breakwater" | "groyne" => return Some(Pier),
            "bridge" => return Some(BridgeArea),
            "wastewater_plant" | "water_works" | "works" => return Some(Industrial),
            "planter" => return Some(Planter),
            _ => {}
        }
    }
    if t.amenity == Some("shelter") {
        return Some(Canopy);
    }
    if t.leisure == Some("bleachers") {
        return Some(Bleachers);
    }
    if t.historic == Some("tomb") {
        return Some(Tomb);
    }
    if t.leisure == Some("swimming_pool") {
        return Some(Pool);
    }
    let water = matches!(t.natural, Some("water"))
        || matches!(t.waterway, Some("riverbank" | "dock"))
        || matches!(t.landuse, Some("reservoir" | "basin" | "salt_pond"))
        || matches!(t.golf, Some("water_hazard" | "lateral_water_hazard"));
    if water {
        return Some(Water);
    }
    if t.waterway == Some("dam") {
        return Some(DamArea);
    }
    if let Some(g) = t.golf {
        match g {
            "bunker" => return Some(GolfBunker),
            "green" => return Some(GolfGreen),
            "tee" => return Some(GolfTee),
            "fairway" => return Some(GolfFairway),
            "rough" => return Some(GolfRough),
            "driving_range" => return Some(Grass),
            _ => {}
        }
    }
    if let Some(a) = t.area_highway {
        return Some(match a {
            "footway" | "pedestrian" | "steps" | "cycleway" => PedestrianArea,
            "traffic_island" => TrafficIsland,
            _ => RoadArea,
        });
    }
    if let Some(a) = t.aeroway {
        match a {
            "aerodrome" => return Some(Aerodrome),
            "apron" => return Some(Apron),
            "runway" | "taxiway" | "stopway" if Tags::yes(t.area) => return Some(RunwayArea),
            "helipad" => return Some(RunwayArea),
            _ => {}
        }
    }
    let platform = matches!(t.railway, Some("platform"))
        || matches!(t.highway, Some("platform"))
        || matches!(t.public_transport, Some("platform"));
    if platform && t.highway != Some("bus_stop") {
        return Some(Platform);
    }
    if matches!(t.highway, Some("pedestrian" | "footway" | "service")) && Tags::yes(t.area)
        || matches!(t.place, Some("square"))
    {
        return Some(PedestrianArea);
    }
    match t.amenity {
        Some("parking_space") => return Some(ParkingSpace),
        Some("fuel") => return Some(Fuel),
        Some("marketplace") => return Some(Retail),
        _ => {}
    }
    if let Some(l) = t.leisure {
        let k = match l {
            "park" | "garden" | "dog_park" | "common" | "recreation_ground" => Some(Park),
            "golf_course" | "miniature_golf" => Some(Golf),
            "pitch" => Some(match t.sport.unwrap_or("") {
                "tennis" | "pickleball" | "padel" => Tennis,
                "basketball" | "american_handball" | "handball" | "volleyball" | "netball" => {
                    HardCourt
                }
                "baseball" | "softball" => Baseball,
                _ => Pitch,
            }),
            "playground" => Some(Playground),
            "sports_centre" | "stadium" | "track" | "ice_rink" | "fitness_station" => Some(Sports),
            "outdoor_seating" => Some(OutdoorSeating),
            _ => None,
        };
        if k.is_some() {
            return k;
        }
    }
    if matches!(t.tourism, Some("zoo" | "theme_park")) {
        return Some(Attraction);
    }
    if let Some(l) = t.landuse {
        let k = match l {
            "residential" => Some(Residential),
            "commercial" => Some(Commercial),
            "retail" => Some(Retail),
            "industrial" | "port" | "depot" => Some(Industrial),
            "railway" => Some(RailwayLand),
            "construction" | "brownfield" | "greenfield" | "landfill" => Some(Construction),
            "cemetery" => Some(Cemetery),
            "military" => Some(Military),
            "garages" => Some(Garages),
            "religious" => Some(Religious),
            "forest" => Some(Forest),
            "grass" | "village_green" | "recreation_ground" | "meadow" | "flowerbed" => Some(Grass),
            "farmland" | "farmyard" => Some(Farmland),
            "orchard" | "vineyard" | "plant_nursery" => Some(Orchard),
            "allotments" => Some(Allotments),
            "quarry" => Some(BareRock),
            "education" => Some(School),
            _ => None,
        };
        if k.is_some() {
            return k;
        }
    }
    if let Some(n) = t.natural {
        let k = match n {
            "wood" => Some(Forest),
            "scrub" | "shrubbery" => Some(Scrub),
            "heath" => Some(Heath),
            "grassland" | "fell" => Some(Grass),
            "wetland" if matches!(t.wetland, Some("tidalflat" | "mud")) => Some(Tidalflat),
            "wetland" => Some(Wetland),
            "mud" => Some(Tidalflat),
            "reef" => Some(Reef),
            "beach" => Some(Beach),
            "sand" | "dune" => Some(Sand),
            "bare_rock" | "scree" | "shingle" | "rock" => Some(BareRock),
            "glacier" => Some(Glacier),
            _ => None,
        };
        if k.is_some() {
            return k;
        }
    }
    if matches!(t.power, Some("substation" | "plant")) {
        return Some(Industrial);
    }
    match t.amenity {
        Some("school" | "university" | "college" | "kindergarten") => Some(School),
        Some("hospital") => Some(Hospital),
        Some("grave_yard") => Some(Cemetery),
        Some("parking") if !matches!(t.parking, Some("underground" | "multi-storey")) => {
            Some(Parking)
        }
        _ => None,
    }
}

/// Classifies an open way (or a closed way that is linear by nature).
pub fn line_kind(t: &Tags) -> Option<Kind> {
    use Kind::*;
    if let Some(h) = t.highway {
        let k = match h.trim_end_matches("_link") {
            "motorway" => Motorway,
            "trunk" => Trunk,
            "primary" => Primary,
            "secondary" => Secondary,
            "tertiary" => Tertiary,
            "residential" | "unclassified" | "living_street" | "road" | "busway" => Minor,
            "pedestrian" => PedestrianStreet,
            "service" => Service,
            "track" => Track,
            "footway" | "path" | "steps" | "bridleway" => Footway,
            "cycleway" => Cycleway,
            _ => return None,
        };
        return Some(k);
    }
    if let Some(r) = t.railway {
        return match r {
            "rail" | "preserved" => Some(Rail),
            "subway" => Some(Subway),
            "light_rail" | "monorail" | "narrow_gauge" | "funicular" | "miniature" => {
                Some(LightRail)
            }
            "tram" => Some(Tram),
            _ => None,
        };
    }
    if let Some(w) = t.waterway {
        if matches!(t.tunnel, Some("culvert" | "yes")) {
            return None;
        }
        return match w {
            "river" => Some(River),
            "canal" => Some(Canal),
            "stream" | "brook" | "tidal_channel" => Some(Stream),
            "ditch" | "drain" => Some(Ditch),
            "dam" => Some(Dam),
            "weir" => Some(Weir),
            _ => None,
        };
    }
    match t.aeroway {
        Some("runway") => return Some(Runway),
        Some("taxiway") => return Some(Taxiway),
        Some("parking_position" | "taxilane" | "holding_position") => return Some(ApronLine),
        Some("jet_bridge") => return Some(JetBridge),
        _ => {}
    }
    match t.man_made {
        Some("pier") => return Some(PierLine),
        Some("breakwater" | "groyne") => return Some(Breakwater),
        Some("embankment") => return Some(Embankment),
        Some("gantry") => return Some(Gantry),
        Some("pipeline") if !matches!(t.location, Some("underground" | "underwater")) => {
            return Some(Pipeline)
        }
        _ => {}
    }
    match t.barrier {
        Some("fence" | "railing") => return Some(Fence),
        Some("wall" | "city_wall" | "retaining_wall") => return Some(Wall),
        Some("hedge") => return Some(Hedge),
        Some("jersey_barrier" | "guard_rail") => return Some(LowBarrier),
        Some("kerb") => return Some(Kerb),
        _ => {}
    }
    match t.natural {
        Some("tree_row") => return Some(TreeRow),
        Some("cliff") => return Some(Cliff),
        _ => {}
    }
    if matches!(t.power, Some("line" | "minor_line")) {
        return Some(PowerLine);
    }
    if t.aerialway.is_some_and(|a| a != "station" && a != "pylon") {
        return Some(Aerialway);
    }
    if t.route == Some("ferry") {
        return Some(Ferry);
    }
    None
}

/// Classifies a way. Closed ways become areas when their tags describe one.
pub fn way_kind(t: &Tags, closed: bool) -> Option<Kind> {
    let linear_by_default = t.highway.is_some()
        || t.barrier.is_some()
        || matches!(t.aeroway, Some("runway" | "taxiway" | "jet_bridge"))
        || matches!(t.railway, Some("rail" | "subway" | "light_rail" | "tram"))
        || matches!(t.natural, Some("tree_row" | "cliff"))
        || t.waterway == Some("weir");
    let area_tags = t.area_highway.is_some() || Tags::yes(t.building_part);
    if closed && t.area != Some("no") && (!linear_by_default || Tags::yes(t.area) || area_tags) {
        if let Some(k) = area_kind(t) {
            return Some(k);
        }
    }
    line_kind(t)
}

/// Classifies a tagged node as a physical object.
pub fn node_kind(t: &Tags) -> Option<Kind> {
    use Kind::*;
    if matches!(t.location, Some("underground" | "indoor")) {
        return None;
    }
    let k = match (t.natural, t.highway, t.amenity, t.man_made) {
        (Some("tree"), ..) => Tree,
        (Some("shrub"), ..) => Shrub,
        (Some("stone" | "rock"), ..) => Stone,
        (_, Some("street_lamp"), ..) => StreetLamp,
        (_, Some("traffic_signals"), ..) => TrafficSignal,
        (_, Some("stop" | "give_way"), ..) => StopSign,
        (_, Some("bus_stop"), ..) if Tags::yes(t.shelter) => Shelter,
        (_, _, Some("bench"), _) => Bench,
        (_, _, Some("waste_basket" | "recycling"), _) => WasteBasket,
        (_, _, Some("post_box"), _) => PostBox,
        (_, _, Some("bicycle_parking"), _) => BicycleParking,
        (_, _, Some("drinking_water" | "fountain" | "water_point"), _) => DrinkingWater,
        (_, _, Some("telephone"), _) => Phone,
        (_, _, Some("shelter"), _) => Shelter,
        (_, _, _, Some("utility_pole")) => PowerPole,
        (_, _, _, Some("flagpole")) => Flagpole,
        (_, _, _, Some("mast")) => Mast,
        (_, _, _, Some("chimney")) => Chimney,
        (_, _, _, Some("tower")) => Tower,
        (_, _, _, Some("water_tower")) => WaterTower,
        (_, _, _, Some("crane")) => Crane,
        (_, _, _, Some("street_cabinet")) => Cabinet,
        _ => match (t.power, t.barrier, t.emergency, t.railway) {
            (Some("pole"), ..) => PowerPole,
            (Some("tower" | "portal"), ..) => PowerTower,
            (_, Some("bollard"), ..) => Bollard,
            (_, Some("block"), ..) => Block,
            (_, Some("toll_booth"), ..) => Cabinet,
            (_, Some("planter"), ..) => Shrub,
            (_, _, Some("fire_hydrant"), _) => Hydrant,
            (_, _, Some("phone"), _) => Phone,
            (_, _, _, Some("subway_entrance")) => SubwayEntrance,
            (_, _, _, Some("signal")) => RailSignal,
            (_, _, _, Some("buffer_stop")) => BufferStop,
            _ => {
                if t.leisure == Some("picnic_table") {
                    PicnicTable
                } else if t.playground.is_some_and(|p| p != "map" && p != "hopscotch") {
                    PlayEquipment
                } else if matches!(t.historic, Some("memorial" | "monument")) {
                    Monument
                } else if t.tourism == Some("artwork") {
                    Artwork
                } else if t.aeroway == Some("windsock") {
                    Windsock
                } else if t.aeroway == Some("navigationaid") {
                    NavLight
                } else {
                    return None;
                }
            }
        },
    };
    Some(k)
}

/// Admin level of an administrative boundary relation worth drawing.
pub fn boundary_level(t: &Tags) -> Option<u8> {
    if t.boundary != Some("administrative") {
        return None;
    }
    t.admin_level
        .and_then(|l| l.parse::<u8>().ok())
        .filter(|&l| (2..=8).contains(&l))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags<'a>(kv: &'a [(&'a str, &'a str)]) -> Tags<'a> {
        Tags::parse(kv.iter().copied())
    }

    #[test]
    fn closed_highway_stays_a_line() {
        let t = tags(&[("highway", "residential")]);
        assert_eq!(way_kind(&t, true), Some(Kind::Minor));
        let t = tags(&[("highway", "pedestrian"), ("area", "yes")]);
        assert_eq!(way_kind(&t, true), Some(Kind::PedestrianArea));
    }

    #[test]
    fn buildings_win_over_amenities() {
        let t = tags(&[("building", "school"), ("amenity", "school")]);
        assert_eq!(way_kind(&t, true), Some(Kind::Building));
        let t = tags(&[("building", "no"), ("amenity", "school")]);
        assert_eq!(way_kind(&t, true), Some(Kind::School));
    }

    #[test]
    fn open_area_tags_are_ignored() {
        let t = tags(&[("landuse", "grass")]);
        assert_eq!(way_kind(&t, false), None);
    }

    #[test]
    fn link_and_bridge_flags() {
        let t = tags(&[
            ("highway", "motorway_link"),
            ("bridge", "yes"),
            ("layer", "2"),
        ]);
        assert_eq!(line_kind(&t), Some(Kind::Motorway));
        assert_eq!(
            t.flags() & (flags::LINK | flags::BRIDGE),
            flags::LINK | flags::BRIDGE
        );
        assert_eq!(t.layer(), 2);
        assert_eq!(
            Kind::Motorway.group(t.layer(), t.flags()),
            Group::TransportBridges
        );
        let t = tags(&[("railway", "subway"), ("tunnel", "yes")]);
        assert_eq!(
            Kind::Subway.group(t.layer(), t.flags()),
            Group::TransportTunnels
        );
    }

    #[test]
    fn heights_parse_units_and_levels() {
        assert_eq!(tags(&[("height", "12.5")]).height(), Some(12.5));
        assert_eq!(tags(&[("height", "30 m")]).height(), Some(30.0));
        let ft = tags(&[("height", "100'")]).height().unwrap();
        assert!((ft - 30.48).abs() < 1e-3);
        assert_eq!(tags(&[("building:levels", "10")]).height(), Some(33.0));
        assert_eq!(tags(&[("min_height", "40")]).min_height(), 40.0);
        assert_eq!(tags(&[("building:min_level", "2")]).min_height(), 6.4);
    }

    #[test]
    fn culverts_are_hidden() {
        let t = tags(&[("waterway", "stream"), ("tunnel", "culvert")]);
        assert_eq!(line_kind(&t), None);
    }

    #[test]
    fn building_parts_and_solids() {
        let t = tags(&[("building:part", "yes"), ("min_height", "40")]);
        assert_eq!(way_kind(&t, true), Some(Kind::Building));
        assert_ne!(t.flags() & flags::PART, 0);
        let t = tags(&[("man_made", "storage_tank")]);
        assert_eq!(way_kind(&t, true), Some(Kind::Tank));
        let t = tags(&[("building", "roof")]);
        assert_eq!(way_kind(&t, true), Some(Kind::Canopy));
        assert_eq!(
            Kind::Building.default_height(&tags(&[("building", "garage")])),
            3.0
        );
    }

    #[test]
    fn physical_nodes() {
        let n = |kv: &[(&str, &str)]| node_kind(&tags(kv));
        assert_eq!(n(&[("highway", "street_lamp")]), Some(Kind::StreetLamp));
        assert_eq!(n(&[("emergency", "fire_hydrant")]), Some(Kind::Hydrant));
        assert_eq!(n(&[("power", "tower")]), Some(Kind::PowerTower));
        assert_eq!(n(&[("amenity", "bench")]), Some(Kind::Bench));
        assert_eq!(n(&[("amenity", "restaurant")]), None);
        assert_eq!(n(&[("highway", "crossing")]), None);
        let t = tags(&[("natural", "tree"), ("leaf_type", "needleleaved")]);
        assert_ne!(t.flags() & flags::CONIFER, 0);
    }

    #[test]
    fn roof_details_and_colours() {
        let t = tags(&[
            ("roof:shape", "gabled"),
            ("roof:height", "3"),
            ("roof:colour", "#a52"),
            ("building:colour", "brown"),
        ]);
        let d = t.detail();
        assert_eq!(d.roof, RoofShape::Gabled);
        assert_eq!(d.roof_height, 3.0);
        assert_eq!(d.roof_colour, Some(0xaa5522));
        assert_eq!(d.facade_colour, Some(0x8a5a3c));
        assert_eq!(parse_colour("#FFFFFF"), Some(0xffffff));
        assert_eq!(parse_colour("plaid"), None);
    }

    #[test]
    fn tower_types() {
        let v = |kv: &[(&str, &str)]| TowerType::from_u8(tags(kv).variant());
        assert_eq!(
            v(&[("man_made", "tower"), ("tower:type", "communication")]),
            TowerType::Monopole
        );
        assert_eq!(
            v(&[("man_made", "tower"), ("tower:construction", "guyed_lattice")]),
            TowerType::Guyed
        );
        assert_eq!(
            v(&[("man_made", "tower"), ("tower:type", "bell_tower")]),
            TowerType::Belfry
        );
        assert_eq!(v(&[("man_made", "mast")]), TowerType::Generic);
    }

    #[test]
    fn pitches_by_sport() {
        let t = tags(&[("leisure", "pitch"), ("sport", "tennis")]);
        assert_eq!(way_kind(&t, true), Some(Kind::Tennis));
        let t = tags(&[("leisure", "pitch"), ("sport", "baseball")]);
        assert_eq!(way_kind(&t, true), Some(Kind::Baseball));
    }
}
