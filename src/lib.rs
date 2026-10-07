//! A fast, parallel OpenStreetMap renderer.
//!
//! Pipeline: [`ingest`] reads an `.osm.pbf` file in three parallel passes,
//! [`map`] assembles and indexes render-ready geometry, and [`render`] draws
//! any viewport with a [`style::Theme`], which [`output`] turns into XYZ tiles
//! or one large poster image.

pub mod api;
pub mod assemble;
pub mod classify;
pub mod elevation;
pub mod geo;
pub mod ingest;
pub mod map;
pub mod output;
pub mod place;
pub mod render;
pub mod server;
pub mod snapshot;
pub mod style;
pub mod vtile;
