//! Turns raw OSM tags into a compact, renderable feature taxonomy.

/// The OSM keys the renderer cares about, pulled out of a tag list in one pass.
#[derive(Default, Debug, Clone, Copy)]
pub struct Tags<'a> {
    pub highway: Option<&'a str>,
    pub railway: Option<&'a str>,
    pub waterway: Option<&'a str>,
    pub building: Option<&'a str>,
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
        {
            f |= flags::MINOR;
        }
        f
    }

    /// Building height in meters, from `height` or `building:levels`.
    pub fn height(&self) -> Option<f32> {
        if let Some(h) = self.height {
            let num: String = h
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = num.parse::<f32>() {
                let feet = h.contains('\'') || h.contains("ft");
                return Some(if feet { v * 0.3048 } else { v });
            }
        }
        self.levels
            .and_then(|l| l.trim().parse::<f32>().ok())
            .map(|l| l * 3.2 + 1.0)
    }
}

/// Bit flags carried alongside a feature's [`Kind`].
pub mod flags {
    pub const BRIDGE: u8 = 1;
    pub const TUNNEL: u8 = 2;
    pub const LINK: u8 = 4;
    pub const MINOR: u8 = 8;
}

/// What a feature is. Ordering within a group matters: it is the draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Kind {
    // Areas.
    Land,
    Water,
    Glacier,
    Wetland,
    Beach,
    Sand,
    BareRock,
    Forest,
    Scrub,
    Heath,
    Grass,
    Park,
    Golf,
    Pitch,
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
    Parking,
    Aerodrome,
    Apron,
    PedestrianArea,
    Pier,
    BridgeArea,
    Platform,
    RunwayArea,
    Building,
    // Lines.
    Ditch,
    Stream,
    Canal,
    River,
    Runway,
    Taxiway,
    PierLine,
    Breakwater,
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
    PowerLine,
    Aerialway,
    Ferry,
    Boundary,
    // Points.
    Tree,
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
    /// Tunnels and anything below ground: under the buildings.
    TransportTunnels,
    Buildings,
    /// Ground-level roads and rails: above buildings so streets stay legible
    /// where footprints touch the street line.
    TransportGround,
    Trees,
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
    pub fn is_area(self) -> bool {
        self <= Kind::Building
    }

    pub fn is_point(self) -> bool {
        self == Kind::Tree
    }

    pub fn is_transport(self) -> bool {
        (Kind::Footway..=Kind::Rail).contains(&self)
    }

    pub fn group(self, layer: i8, f: u8) -> Group {
        use Kind::*;
        match self {
            Land => Group::Land,
            Pier | BridgeArea | Platform | RunwayArea => Group::AreaOverlays,
            Building => Group::Buildings,
            k if k.is_area() => Group::Areas,
            Ditch | Stream | Canal | River => Group::Waterways,
            Runway | Taxiway | PierLine | Breakwater => Group::GroundLines,
            k if k.is_transport() && (layer < 0 || f & flags::TUNNEL != 0) => {
                Group::TransportTunnels
            }
            k if k.is_transport() && layer > 0 => Group::TransportBridges,
            k if k.is_transport() => Group::TransportGround,
            Tree => Group::Trees,
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
            Land => 0.0,
            Water => 4.0,
            Glacier | Forest | Wetland => 6.0,
            Farmland | Residential | Industrial | Commercial | Retail | Grass | Park | Scrub
            | Heath | Beach | Sand | BareRock | Aerodrome | Military | RailwayLand | Cemetery
            | Golf | Orchard => 8.0,
            Construction | Allotments | Religious | School | Hospital | Garages | Apron => 10.0,
            Sports | Pitch | RunwayArea => 12.0,
            Playground | Parking | PedestrianArea | Pier | BridgeArea | Platform => 13.0,
            Building => 13.0,
            River => 8.0,
            Canal => 10.0,
            Stream => 13.0,
            Ditch => 15.0,
            Runway => 10.0,
            Taxiway => 12.0,
            PierLine | Breakwater => 14.0,
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
            Aerialway | PowerLine => 14.0,
            Hedge | Fence | Wall => 16.0,
            Tree => 16.0,
        }
    }
}

/// Classifies a closed ring (way or multipolygon) as an area kind.
pub fn area_kind(t: &Tags) -> Option<Kind> {
    use Kind::*;
    if matches!(t.location, Some("underground")) || t.layer() < 0 && t.building.is_some() {
        return None;
    }
    if let Some(b) = t.building {
        if b != "no" {
            return Some(Building);
        }
    }
    if matches!(t.aeroway, Some("terminal" | "hangar")) {
        return Some(Building);
    }
    let water = matches!(t.natural, Some("water"))
        || matches!(t.waterway, Some("riverbank" | "dock"))
        || matches!(t.landuse, Some("reservoir" | "basin"))
        || matches!(t.leisure, Some("swimming_pool" | "marina"));
    if water {
        return Some(Water);
    }
    if let Some(a) = t.aeroway {
        match a {
            "aerodrome" => return Some(Aerodrome),
            "apron" => return Some(Apron),
            "runway" | "taxiway" if Tags::yes(t.area) => return Some(RunwayArea),
            "helipad" => return Some(RunwayArea),
            _ => {}
        }
    }
    if let Some(m) = t.man_made {
        match m {
            "pier" | "breakwater" | "groyne" => return Some(Pier),
            "bridge" => return Some(BridgeArea),
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
    if let Some(l) = t.leisure {
        let k = match l {
            "park" | "garden" | "dog_park" | "common" | "recreation_ground" => Some(Park),
            "golf_course" | "miniature_golf" => Some(Golf),
            "pitch" => Some(Pitch),
            "playground" => Some(Playground),
            "sports_centre" | "stadium" | "track" | "ice_rink" | "fitness_station" => Some(Sports),
            _ => None,
        };
        if k.is_some() {
            return k;
        }
    }
    if let Some(l) = t.landuse {
        let k = match l {
            "residential" => Some(Residential),
            "commercial" => Some(Commercial),
            "retail" => Some(Retail),
            "industrial" | "port" => Some(Industrial),
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
            "scrub" => Some(Scrub),
            "heath" => Some(Heath),
            "grassland" | "fell" => Some(Grass),
            "wetland" | "mud" => Some(Wetland),
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
            "stream" | "brook" => Some(Stream),
            "ditch" | "drain" => Some(Ditch),
            _ => None,
        };
    }
    match t.aeroway {
        Some("runway") => return Some(Runway),
        Some("taxiway") => return Some(Taxiway),
        _ => {}
    }
    match t.man_made {
        Some("pier") => return Some(PierLine),
        Some("breakwater" | "groyne") => return Some(Breakwater),
        _ => {}
    }
    match t.barrier {
        Some("fence" | "guard_rail" | "railing") => return Some(Fence),
        Some("wall" | "city_wall" | "retaining_wall") => return Some(Wall),
        Some("hedge") => return Some(Hedge),
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
        || matches!(t.aeroway, Some("runway" | "taxiway"))
        || matches!(t.railway, Some("rail" | "subway" | "light_rail" | "tram"));
    if closed && t.area != Some("no") && (!linear_by_default || Tags::yes(t.area)) {
        if let Some(k) = area_kind(t) {
            return Some(k);
        }
    }
    line_kind(t)
}

/// Classifies a tagged node.
pub fn node_kind(t: &Tags) -> Option<Kind> {
    (t.natural == Some("tree")).then_some(Kind::Tree)
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
    }

    #[test]
    fn culverts_are_hidden() {
        let t = tags(&[("waterway", "stream"), ("tunnel", "culvert")]);
        assert_eq!(line_kind(&t), None);
    }
}
