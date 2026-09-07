//! Native GPUI painting for mmdr's backend-neutral vector scenes.
//!
//! There is no image primitive, pixel buffer, SVG decoder, or texture cache in
//! this module. The upstream scene normalizes geometry and outlines text. We
//! tessellate its paths once, then transform those vertices for the actual
//! transcript bounds, preserving crisp output at every display scale.
//!
//! GPUI has no isolated group or multiply blend primitive. Group opacity is
//! applied per path and multiply uses source-over. This only approximates
//! overlapping translucent groups (not geometry). Clips intersect tessellated
//! geometry directly. Unsupported nonhorizontal/multistop gradients return
//! errors, never bitmaps.
use std::sync::Arc;

use gpui::{
    AnyElement, Background, Bounds, ContentMask, FillOptions, Path, PathBuilder, PathStyle, Pixels,
    Rgba, canvas, div, linear_color_stop, linear_gradient, point, prelude::*, px, size,
};
use mermaid_rs_renderer::scene::{
    BlendMode, Color, FillRule, Paint, PathCommand, Scene, SceneCommand,
};

pub(crate) const MAX_DIAGRAM_HEIGHT: f32 = 520.;

/// A cached, tessellated vector diagram. Cloning does not duplicate geometry.
#[derive(Clone)]
pub(crate) struct NativeMermaid {
    width: f32,
    height: f32,
    paths: Arc<[NativePath]>,
}

struct NativeClip {
    bounds: Bounds<Pixels>,
    mesh: Option<Path<Pixels>>,
}

struct NativePath {
    path: Path<Pixels>,
    paint: Background,
    clip: Option<Bounds<Pixels>>,
}

impl NativeMermaid {
    pub(crate) fn new(scene: &Scene) -> Result<Self, String> {
        if !scene.width.is_finite()
            || !scene.height.is_finite()
            || scene.width <= 0.
            || scene.height <= 0.
        {
            return Err("Mermaid scene has invalid dimensions".into());
        }
        let mut paths = Vec::with_capacity(scene.commands.len());
        let mut clips: Vec<NativeClip> = Vec::new();
        let mut layers = vec![1f32];
        for command in &scene.commands {
            let (path, paint, fill_rule) = match command {
                SceneCommand::FillPath {
                    path,
                    paint,
                    fill_rule,
                } => (path, paint, fill_rule),
                SceneCommand::PushClip { path, fill_rule } => {
                    let (mut bounds, mesh) = match rectangular_clip(path) {
                        Ok(bounds) => (bounds, None),
                        Err(_) => {
                            let mesh = tessellate(path, *fill_rule)?;
                            (mesh.bounds, Some(mesh))
                        }
                    };
                    if let Some(parent) = clips.last() {
                        bounds = bounds.intersect(&parent.bounds);
                    }
                    clips.push(NativeClip { bounds, mesh });
                    continue;
                }
                SceneCommand::PopClip => {
                    clips.pop().ok_or("Unbalanced Mermaid clip stack")?;
                    continue;
                }
                SceneCommand::PushLayer {
                    opacity,
                    blend_mode,
                } => {
                    // GPUI exposes source-over primitives, not isolated groups.
                    // Preserve native vector output, approximating group alpha
                    // per primitive and multiply blending with source-over.
                    match blend_mode {
                        BlendMode::Normal | BlendMode::Multiply => {}
                    }
                    layers.push(layers.last().copied().unwrap_or(1.) * opacity);
                    continue;
                }
                SceneCommand::PopLayer => {
                    if layers.len() <= 1 {
                        return Err("Unbalanced Mermaid layer stack".into());
                    }
                    layers.pop();
                    continue;
                }
            };
            let mut path = tessellate(path, *fill_rule)?;
            for clip in &clips {
                if let Some(mesh) = &clip.mesh {
                    clip_geometry(&mut path, mesh);
                }
            }
            let paint = native_paint(paint, path.bounds)?.opacity(*layers.last().unwrap());
            paths.push(NativePath {
                path,
                paint,
                clip: clips.last().map(|clip| clip.bounds),
            });
        }
        if !clips.is_empty() || layers.len() != 1 {
            return Err("Unbalanced Mermaid scene stacks".into());
        }
        Ok(Self {
            width: scene.width,
            height: scene.height,
            paths: paths.into(),
        })
    }

    #[cfg(test)]
    pub(crate) fn width(&self) -> f32 {
        self.width
    }
    #[cfg(test)]
    pub(crate) fn height(&self) -> f32 {
        self.height
    }

    /// Intrinsic size is an upper bound, never a target to upscale toward.
    /// Width and aspect ratio drive Taffy layout together, so tall diagrams
    /// reserve precisely their scaled height rather than a clipped fixed box.
    pub(crate) fn element(&self) -> AnyElement {
        let diagram = self.clone();
        let max_width = self
            .width
            .min(MAX_DIAGRAM_HEIGHT * self.width / self.height);
        let canvas = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let transform = fit(diagram.width, diagram.height, bounds);
                if transform.scale <= 0. {
                    return;
                }
                for primitive in diagram.paths.iter() {
                    let mut path = primitive.path.clone();
                    path.bounds.origin = transform.point(path.bounds.origin);
                    path.bounds.size.width *= transform.scale;
                    path.bounds.size.height *= transform.scale;
                    for vertex in &mut path.vertices {
                        vertex.xy_position = transform.point(vertex.xy_position);
                    }
                    let mask = primitive.clip.map(|clip| ContentMask {
                        bounds: Bounds::new(
                            transform.point(clip.origin),
                            size(
                                clip.size.width * transform.scale,
                                clip.size.height * transform.scale,
                            ),
                        ),
                    });
                    window
                        .with_content_mask(mask, |window| window.paint_path(path, primitive.paint));
                }
            },
        )
        .size_full();
        div()
            .debug_selector(|| "md-mermaid-native".into())
            .w_full()
            .max_w(px(max_width))
            .max_h(px(self.height.min(MAX_DIAGRAM_HEIGHT)))
            .aspect_ratio(self.width / self.height)
            .flex_none()
            .child(canvas)
            .into_any_element()
    }
}

fn tessellate(path: &[PathCommand], fill_rule: FillRule) -> Result<Path<Pixels>, String> {
    let fill_rule = match fill_rule {
        FillRule::NonZero => gpui::FillRule::NonZero,
        FillRule::EvenOdd => gpui::FillRule::EvenOdd,
    };
    let mut builder = PathBuilder::fill().with_style(PathStyle::Fill(
        FillOptions::default().with_fill_rule(fill_rule),
    ));
    for command in path {
        match *command {
            PathCommand::MoveTo { x, y } => builder.move_to(point(px(x), px(y))),
            PathCommand::LineTo { x, y } => builder.line_to(point(px(x), px(y))),
            PathCommand::QuadTo { x1, y1, x, y } => {
                builder.curve_to(point(px(x), px(y)), point(px(x1), px(y1)));
            }
            PathCommand::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                builder.cubic_bezier_to(
                    point(px(x), px(y)),
                    point(px(x1), px(y1)),
                    point(px(x2), px(y2)),
                );
            }
            PathCommand::Close => builder.close(),
        }
    }
    builder.build().map_err(|e| format!("Mermaid path: {e}"))
}

/// Intersect flat triangle meshes once at scene preparation, not every frame.
/// usvg may introduce rotated marker viewport clips even when the source only
/// declares rectangles. This also honors curved clips and holes using the same
/// fill-rule tessellation as visible paths, without any offscreen pixel layer.
fn clip_geometry(path: &mut Path<Pixels>, clip: &Path<Pixels>) {
    let masks: Vec<_> = clip
        .vertices
        .chunks_exact(3)
        .filter_map(|triangle| {
            let points = [
                triangle[0].xy_position,
                triangle[1].xy_position,
                triangle[2].xy_position,
            ];
            let area = cross(points[1] - points[0], points[2] - points[0]);
            (area.abs() > 0.000001).then(|| (points, polygon_bounds(&points), area.signum()))
        })
        .collect();
    let mut vertices = Vec::new();
    for triangle in path.vertices.chunks_exact(3) {
        let subject: Vec<_> = triangle.iter().map(|v| v.xy_position).collect();
        let subject_bounds = polygon_bounds(&subject);
        for (mask, mask_bounds, orientation) in &masks {
            let intersection = subject_bounds.intersect(mask_bounds);
            if intersection.size.width <= px(0.) || intersection.size.height <= px(0.) {
                continue;
            }
            let mut polygon = subject.clone();
            for i in 0..3 {
                if polygon.is_empty() {
                    break;
                }
                let a = mask[i];
                let edge = mask[(i + 1) % 3] - a;
                let mut output = Vec::new();
                let mut previous = *polygon.last().unwrap();
                let mut previous_distance = cross(edge, previous - a) * orientation;
                for current in polygon {
                    let distance = cross(edge, current - a) * orientation;
                    if (distance >= 0.) != (previous_distance >= 0.) {
                        let fraction = previous_distance / (previous_distance - distance);
                        output.push(previous + (current - previous) * fraction);
                    }
                    if distance >= 0. {
                        output.push(current);
                    }
                    previous = current;
                    previous_distance = distance;
                }
                polygon = output;
            }
            for i in 1..polygon.len().saturating_sub(1) {
                if cross(polygon[i] - polygon[0], polygon[i + 1] - polygon[0]).abs() < 0.000001 {
                    continue;
                }
                for position in [polygon[0], polygon[i], polygon[i + 1]] {
                    let mut vertex = triangle[0].clone();
                    vertex.xy_position = position;
                    vertices.push(vertex);
                }
            }
        }
    }
    path.vertices = vertices;
    // Keep the original bounds: gradient stops are relative to the original
    // paint box, not the smaller geometry surviving the clip.
}

fn cross(a: gpui::Point<Pixels>, b: gpui::Point<Pixels>) -> f32 {
    f32::from(a.x) * f32::from(b.y) - f32::from(a.y) * f32::from(b.x)
}

fn polygon_bounds(points: &[gpui::Point<Pixels>]) -> Bounds<Pixels> {
    let left = points.iter().map(|p| p.x).min().unwrap();
    let top = points.iter().map(|p| p.y).min().unwrap();
    let right = points.iter().map(|p| p.x).max().unwrap();
    let bottom = points.iter().map(|p| p.y).max().unwrap();
    Bounds::new(point(left, top), size(right - left, bottom - top))
}

fn color(color: Color) -> Rgba {
    Rgba {
        r: color.r as f32 / 255.,
        g: color.g as f32 / 255.,
        b: color.b as f32 / 255.,
        a: color.a,
    }
}

fn native_paint(paint: &Paint, bounds: Bounds<Pixels>) -> Result<Background, String> {
    match paint {
        Paint::Solid(value) => Ok(color(*value).into()),
        Paint::LinearGradient { start, end, stops } => {
            if stops.len() != 2 || (start.1 - end.1).abs() > 0.001 {
                return Err("Native Mermaid supports horizontal two-stop gradients".into());
            }
            let width = f32::from(bounds.size.width);
            if width <= 0. {
                return Ok(color(stops[0].color).into());
            }
            let offset = |offset: f32| {
                (start.0 + (end.0 - start.0) * offset - f32::from(bounds.left())) / width
            };
            let mut first = linear_color_stop(color(stops[0].color), offset(stops[0].offset));
            let mut last = linear_color_stop(color(stops[1].color), offset(stops[1].offset));
            if first.percentage > last.percentage {
                std::mem::swap(&mut first, &mut last);
            }
            Ok(linear_gradient(90., first, last))
        }
    }
}

/// GPUI content masks are rectangular. Reject arbitrary contours rather than
/// silently replacing their clipping semantics with a bounding box.
fn rectangular_clip(path: &[PathCommand]) -> Result<Bounds<Pixels>, String> {
    let mut points = Vec::new();
    for command in path {
        match command {
            PathCommand::MoveTo { x, y } if points.is_empty() => points.push((*x, *y)),
            PathCommand::LineTo { x, y } => points.push((*x, *y)),
            PathCommand::Close => {}
            _ => return Err("Native Mermaid requires rectangular clips".into()),
        }
    }
    if points.len() == 5 && points.first() == points.last() {
        points.pop();
    }
    if points.len() != 4 {
        return Err("Native Mermaid requires rectangular clips".into());
    }
    let left = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let top = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let right = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let bottom = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    if ![(left, top), (right, top), (right, bottom), (left, bottom)]
        .iter()
        .all(|corner| points.contains(corner))
    {
        return Err("Native Mermaid requires rectangular clips".into());
    }
    for i in 0..4 {
        let (x, y) = points[i];
        let next = points[(i + 1) % 4];
        if !x.is_finite()
            || !y.is_finite()
            || (x != left && x != right)
            || (y != top && y != bottom)
            || (x != next.0 && y != next.1)
        {
            return Err("Native Mermaid requires rectangular clips".into());
        }
    }
    Ok(Bounds::new(
        point(px(left), px(top)),
        size(px(right - left), px(bottom - top)),
    ))
}

#[derive(Debug)]
struct Fit {
    scale: f32,
    origin: gpui::Point<Pixels>,
}

impl Fit {
    fn point(&self, value: gpui::Point<Pixels>) -> gpui::Point<Pixels> {
        self.origin + value * self.scale
    }
}

fn fit(width: f32, height: f32, bounds: Bounds<Pixels>) -> Fit {
    let scale = (f32::from(bounds.size.width) / width)
        .min(f32::from(bounds.size.height) / height)
        .min(1.)
        .max(0.);
    Fit {
        scale,
        origin: bounds.origin
            + point(
                (bounds.size.width - px(width * scale)) / 2.,
                (bounds.size.height - px(height * scale)) / 2.,
            ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn small_diagrams_are_centered_without_upscaling() {
        let fitted = fit(
            100.,
            80.,
            Bounds::new(point(px(10.), px(20.)), size(px(600.), px(520.))),
        );
        assert_eq!(fitted.scale, 1.);
        assert_eq!(fitted.origin, point(px(260.), px(240.)));
    }

    #[test]
    fn tall_diagrams_fit_height_without_clipping() {
        let fitted = fit(
            400.,
            1600.,
            Bounds::new(point(px(0.), px(0.)), size(px(130.), px(520.))),
        );
        assert_eq!(fitted.scale, 0.325);
        assert_eq!(
            fitted.point(point(px(400.), px(1600.))),
            point(px(130.), px(520.))
        );
    }

    #[test]
    fn narrow_transcript_scales_both_axes() {
        let fitted = fit(
            1000.,
            500.,
            Bounds::new(point(px(3.), px(7.)), size(px(300.), px(150.))),
        );
        assert_eq!(fitted.scale, 0.3);
        assert_eq!(
            fitted.point(point(px(1000.), px(500.))),
            point(px(303.), px(157.))
        );
    }

    #[test]
    fn empty_bounds_have_no_paintable_area() {
        let fitted = fit(
            100.,
            80.,
            Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.))),
        );
        assert_eq!(fitted.scale, 0.);
    }

    fn rectangle(left: f32, top: f32, right: f32, bottom: f32) -> Vec<PathCommand> {
        vec![
            PathCommand::MoveTo { x: left, y: top },
            PathCommand::LineTo { x: right, y: top },
            PathCommand::LineTo {
                x: right,
                y: bottom,
            },
            PathCommand::LineTo { x: left, y: bottom },
            PathCommand::Close,
        ]
    }

    fn solid_path(path: Vec<PathCommand>, fill_rule: FillRule) -> SceneCommand {
        SceneCommand::FillPath {
            path,
            fill_rule,
            paint: Paint::Solid(Color {
                r: 40,
                g: 80,
                b: 120,
                a: 0.5,
            }),
        }
    }

    #[test]
    fn glyph_holes_preserve_even_odd_and_nonzero_fill_rules() {
        let mut path = rectangle(0., 0., 10., 10.);
        path.extend(rectangle(3., 3., 7., 7.));
        for (rule, expected_area) in [(FillRule::EvenOdd, 84.), (FillRule::NonZero, 100.)] {
            let scene = Scene {
                width: 10.,
                height: 10.,
                commands: vec![solid_path(path.clone(), rule)],
            };
            let native = NativeMermaid::new(&scene).unwrap();
            let area: f32 = native.paths[0]
                .path
                .vertices
                .chunks_exact(3)
                .map(|triangle| {
                    let a = triangle[0].xy_position;
                    let b = triangle[1].xy_position;
                    let c = triangle[2].xy_position;
                    ((f32::from(b.x - a.x) * f32::from(c.y - a.y)
                        - f32::from(b.y - a.y) * f32::from(c.x - a.x))
                        / 2.)
                        .abs()
                })
                .sum();
            assert!(
                (area - expected_area).abs() < 0.001,
                "area={area}, expected={expected_area}"
            );
        }
    }

    #[test]
    fn native_geometry_is_shareable_without_copying() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NativeMermaid>();
        let scene = Scene {
            width: 10.,
            height: 10.,
            commands: vec![solid_path(rectangle(0., 0., 10., 10.), FillRule::NonZero)],
        };
        let native = NativeMermaid::new(&scene).unwrap();
        assert!(Arc::ptr_eq(&native.paths, &native.clone().paths));
        assert_eq!(
            color(Color {
                r: 0,
                g: 255,
                b: 0,
                a: 0.25
            })
            .a,
            0.25
        );
    }

    #[test]
    fn nested_rectangular_clips_intersect_and_restore() {
        let scene = Scene {
            width: 100.,
            height: 100.,
            commands: vec![
                SceneCommand::PushClip {
                    path: rectangle(0., 0., 80., 80.),
                    fill_rule: FillRule::NonZero,
                },
                SceneCommand::PushClip {
                    path: rectangle(20., 20., 100., 100.),
                    fill_rule: FillRule::NonZero,
                },
                solid_path(rectangle(0., 0., 100., 100.), FillRule::NonZero),
                SceneCommand::PopClip,
                solid_path(rectangle(0., 0., 100., 100.), FillRule::NonZero),
                SceneCommand::PopClip,
            ],
        };
        let native = NativeMermaid::new(&scene).unwrap();
        assert_eq!(
            native.paths[0].clip.unwrap(),
            Bounds::new(point(px(20.), px(20.)), size(px(60.), px(60.)))
        );
        assert_eq!(
            native.paths[1].clip.unwrap(),
            Bounds::new(point(px(0.), px(0.)), size(px(80.), px(80.)))
        );
    }

    #[test]
    fn unsupported_clips_and_invalid_scenes_fail_explicitly() {
        assert!(
            rectangular_clip(&[
                PathCommand::MoveTo { x: 0., y: 0. },
                PathCommand::LineTo { x: 10., y: 5. },
                PathCommand::LineTo { x: 0., y: 10. },
                PathCommand::Close
            ])
            .is_err()
        );
        for dimension in [0., -1., f32::NAN, f32::INFINITY] {
            assert!(
                NativeMermaid::new(&Scene {
                    width: dimension,
                    height: 100.,
                    commands: vec![]
                })
                .is_err()
            );
        }
        assert!(
            NativeMermaid::new(&Scene {
                width: 100.,
                height: 100.,
                commands: vec![SceneCommand::PopClip]
            })
            .is_err()
        );
        assert!(
            NativeMermaid::new(&Scene {
                width: 100.,
                height: 100.,
                commands: vec![SceneCommand::PopLayer]
            })
            .is_err()
        );
    }

    #[test]
    fn every_mmdr_diagram_family_tessellates_natively() {
        // Representative syntax from mmdr's family-level layout fixtures. These
        // exercise the public scene API and native tessellation together,
        // including XY clipping, Sankey gradients, strokes, and outlined text.
        let diagrams = [
            (
                "architecture",
                "architecture-beta\n  group api(icon)[API]\n  service web(icon)[Web] in api\n  service db(icon)[DB] in api\n  web:R --> L:db\n",
            ),
            (
                "block",
                "block\n  A --> B\n  A --> C\n  B --> D\n  C --> D\n",
            ),
            (
                "c4",
                "C4Context\n  Person(admin, \"Admin\")\n  System(sys, \"System\")\n  Rel(admin, sys, \"Uses\")\n  Boundary(b0, \"Boundary\") {\n    SystemDb(db, \"DB\")\n  }\n",
            ),
            (
                "class",
                "classDiagram\nclass Animal {\n+String name\n+eat()\n}\nclass Dog\nAnimal <|-- Dog : inherits\n",
            ),
            (
                "er",
                "erDiagram\nCUSTOMER ||--o{ ORDER : places\nCUSTOMER {\nstring id\nstring name\n}\nORDER {\nstring id\ndate created_at\n}\n",
            ),
            (
                "flowchart",
                "flowchart LR\n  A([Start]) --> B{Decision}\n  B -->|yes| C[/Do thing/]\n  B -->|no| D[\\\\Skip\\\\]\n  C --> E[[End]]\n  D --> E\n",
            ),
            (
                "gantt",
                "gantt\n  title Plan\n  dateFormat  YYYY-MM-DD\n  section Alpha\n  Task A : done, a1, 2026-01-01, 5d\n  Task B : after a1, 3d\n",
            ),
            (
                "gitgraph",
                "gitGraph\n  commit\n  branch feature\n  checkout feature\n  commit id:\"F1\"\n  checkout main\n  merge feature\n",
            ),
            (
                "journey",
                "journey\n  title My Journey\n  section Start\n    Step one: 5: Alice\n    Step two: 3: Alice, Bob\n",
            ),
            (
                "kanban",
                "kanban\n  todo[To Do]\n    t1[Task 1]\n  done[Done]\n    t2[Task 2]\n",
            ),
            (
                "mindmap",
                "mindmap\n  root((Root))\n    Child A\n    Child B\n      Grandchild\n",
            ),
            ("packet", "packet\n  0-7: \"Type\"\n  8-15: \"Len\"\n"),
            (
                "pie",
                "pie showData\n  title Pets\n  \"Dogs\" : 10\n  Cats : 5\n",
            ),
            (
                "quadrant",
                "quadrantChart\n  title Sample\n  x-axis Low Reach --> High Reach\n  y-axis Low Engagement --> High Engagement\n  quadrant-1 Execute\n  quadrant-2 Expand\n  quadrant-3 Monitor\n  quadrant-4 Re-evaluate\n  A : [0.2, 0.8]\n  B : [0.7, 0.3]\n",
            ),
            (
                "radar",
                "radar-beta\n  axis A, B, C\n  curve Alpha {1,2,3}\n",
            ),
            (
                "requirement",
                "requirementDiagram\n  requirement req1 {\n    id: 1\n    text: Login\n    risk: medium\n    verifymethod: test\n  }\n  requirement req2 {\n    id: 2\n    text: Session\n    risk: low\n    verifymethod: inspection\n  }\n  req1 - satisfies -> req2\n",
            ),
            ("sankey", "sankey\n  A, B, 10\n  B, C, 5\n  A, C, 2\n"),
            (
                "sequence",
                "sequenceDiagram\n  participant Alice\n  participant Bob\n  Alice->>Bob: Hello Bob\n  Bob-->>Alice: Hi Alice\n",
            ),
            (
                "state",
                "stateDiagram-v2\n[*] --> Idle\nIdle --> Active : start\nstate \"Waiting\" as Wait\nWait --> Active\n",
            ),
            (
                "timeline",
                "timeline\n  title History\n  2020 : Launch\n  2021 : Growth\n",
            ),
            ("treemap", "treemap-beta\n  Root: 100\n    Child: 40\n"),
            (
                "xychart",
                "xychart-beta\n  x-axis [Q1, Q2]\n  y-axis Units\n  bar [10, 20]\n",
            ),
            (
                "zenuml",
                "zenuml\n  Alice->Bob: Hello\n  Bob-->Alice: Reply\n",
            ),
        ];
        for (family, source) in diagrams {
            let scene = mermaid_rs_renderer::render_scene(source, Default::default())
                .unwrap_or_else(|error| panic!("{family} scene: {error}"));
            let native = NativeMermaid::new(&scene)
                .unwrap_or_else(|error| panic!("{family} native tessellation: {error}"));
            assert!(
                !native.paths.is_empty(),
                "{family} produced no native paths"
            );
            assert!(
                native.paths.iter().any(|p| !p.path.vertices.is_empty()),
                "{family} has no painted geometry"
            );
        }
    }

    #[test]
    fn native_mesh_clipping_preserves_rotated_markers_and_holes() {
        let diamond = vec![
            PathCommand::MoveTo { x: 5., y: 0. },
            PathCommand::LineTo { x: 10., y: 5. },
            PathCommand::LineTo { x: 5., y: 10. },
            PathCommand::LineTo { x: 0., y: 5. },
            PathCommand::Close,
        ];
        let mut hole = rectangle(0., 0., 10., 10.);
        hole.extend(rectangle(3., 3., 7., 7.));
        for (clip, rule, expected_area) in [
            (diamond, FillRule::NonZero, 50.),
            (hole, FillRule::EvenOdd, 84.),
        ] {
            let scene = Scene {
                width: 10.,
                height: 10.,
                commands: vec![
                    SceneCommand::PushClip {
                        path: clip,
                        fill_rule: rule,
                    },
                    solid_path(rectangle(0., 0., 10., 10.), FillRule::NonZero),
                    SceneCommand::PopClip,
                ],
            };
            let native = NativeMermaid::new(&scene).unwrap();
            let area: f32 = native.paths[0]
                .path
                .vertices
                .chunks_exact(3)
                .map(|t| {
                    cross(
                        t[1].xy_position - t[0].xy_position,
                        t[2].xy_position - t[0].xy_position,
                    )
                    .abs()
                        / 2.
                })
                .sum();
            assert!(
                (area - expected_area).abs() < 0.001,
                "clipped area={area}, expected={expected_area}"
            );
        }
    }
}
