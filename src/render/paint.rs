//! Small tiny-skia helpers shared by the drawing modules.

use crate::geo::{Piece, Point};
use crate::style::StrokeSpec;
use tiny_skia::{
    Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform,
};

/// Transformed, clipped line geometry for one style batch.
#[derive(Default)]
pub(super) struct LineGeom {
    pub(super) points: Vec<Point>,
    pub(super) pieces: Vec<Piece>,
}

pub(super) fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

pub(super) fn stroke(spec: &StrokeSpec, dash_offset: f64) -> Stroke {
    Stroke {
        width: spec.width,
        line_cap: if spec.round {
            LineCap::Round
        } else {
            LineCap::Butt
        },
        line_join: if spec.round {
            LineJoin::Round
        } else {
            LineJoin::Miter
        },
        dash: spec.dash.and_then(|[on, off]| {
            let phase = dash_offset.rem_euclid((on + off) as f64) as f32;
            StrokeDash::new(vec![on, off], phase)
        }),
        ..Stroke::default()
    }
}

pub(super) fn push_polyline(pb: &mut PathBuilder, pts: &[Point]) {
    pb.move_to(pts[0][0] as f32, pts[0][1] as f32);
    for p in &pts[1..] {
        pb.line_to(p[0] as f32, p[1] as f32);
    }
}

pub(super) fn push_ring(pb: &mut PathBuilder, pts: &[Point]) {
    if pts.len() >= 3 {
        push_polyline(pb, pts);
        pb.close();
    }
}

/// Strokes clipped line pieces. Solid strokes go out as one path; dashed ones
/// stroke per piece, offset by the distance travelled so far along the
/// original line, so dash patterns stay continuous across tile edges.
pub(super) fn stroke_pieces(
    pixmap: &mut Pixmap,
    geom: &LineGeom,
    spec: &StrokeSpec,
    transform: Transform,
) {
    stroke_pieces_with(pixmap, geom, spec, transform, false);
}

/// Like [`stroke_pieces`]; `butt` forces flat line ends (joins stay as
/// specified), so consecutive chunks of one line abut without overlapping.
pub(super) fn stroke_pieces_with(
    pixmap: &mut Pixmap,
    geom: &LineGeom,
    spec: &StrokeSpec,
    transform: Transform,
    butt: bool,
) {
    let p = paint(spec.color);
    let stroke = |spec: &StrokeSpec, offset: f64| {
        let mut st = stroke(spec, offset);
        if butt {
            st.line_cap = tiny_skia::LineCap::Butt;
        }
        st
    };
    if spec.dash.is_none() {
        let mut pb = PathBuilder::new();
        for piece in &geom.pieces {
            push_polyline(&mut pb, &geom.points[piece.start..piece.start + piece.len]);
        }
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &p, &stroke(spec, 0.0), transform, None);
        }
        return;
    }
    for piece in &geom.pieces {
        let mut pb = PathBuilder::new();
        push_polyline(&mut pb, &geom.points[piece.start..piece.start + piece.len]);
        if let Some(path) = pb.finish() {
            let st = stroke(spec, piece.distance);
            pixmap.stroke_path(&path, &p, &st, transform, None);
        }
    }
}
