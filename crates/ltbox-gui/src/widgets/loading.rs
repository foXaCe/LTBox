//! CPU vector approximation of Material's seven loading shapes for iced.
//! Sequence and motion reference: AndroidX Material3 LoadingIndicator.kt
//! https://android.googlesource.com/platform/frameworks/support/+/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/LoadingIndicator.kt
//! (650ms morphs, damping 0.6/stiffness 200, 4666ms global rotation).
//! The rounded outlines below are LTBox geometry, not AndroidX polygon data.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::OnceLock;

type Point = (f32, f32);
const SAMPLES: usize = 160;
const INTERVAL: f32 = 0.650;

fn rounded_polygon(vertices: &[Point], cut: f32) -> Vec<Point> {
    let mut outline = Vec::new();
    for i in 0..vertices.len() {
        let prev = vertices[(i + vertices.len() - 1) % vertices.len()];
        let corner = vertices[i];
        let next = vertices[(i + 1) % vertices.len()];
        let a = lerp(corner, prev, cut);
        let b = lerp(corner, next, cut);
        for j in 0..=12 {
            let t = j as f32 / 12.0;
            outline.push(lerp(lerp(a, corner, t), lerp(corner, b, t), t));
        }
    }
    outline
}

fn lerp(a: Point, b: Point, t: f32) -> Point {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

fn polygon(tips: usize, inner: Option<f32>, cut: f32) -> Vec<Point> {
    let count = tips * if inner.is_some() { 2 } else { 1 };
    let vertices: Vec<_> = (0..count)
        .map(|i| {
            let angle = TAU * i as f32 / count as f32 - FRAC_PI_2;
            let radius = if i % 2 == 1 {
                inner.unwrap_or(1.0)
            } else {
                1.0
            };
            (radius * angle.cos(), radius * angle.sin())
        })
        .collect();
    rounded_polygon(&vertices, cut)
}

fn ellipse(power: f32, aspect: f32) -> Vec<Point> {
    (0..SAMPLES)
        .map(|i| {
            let angle = TAU * i as f32 / SAMPLES as f32 - FRAC_PI_2;
            let (s, c) = angle.sin_cos();
            (
                c.signum() * c.abs().powf(2.0 / power) * aspect,
                s.signum() * s.abs().powf(2.0 / power),
            )
        })
        .collect()
}

// Equal perimeter sampling gives all outlines corresponding points, avoiding
// changes in vertex count and uneven motion when morphing a star into an oval.
fn resample(points: &[Point]) -> Vec<Point> {
    let mut lengths = vec![0.0];
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        lengths.push(lengths[i] + (b.0 - a.0).hypot(b.1 - a.1));
    }
    let perimeter = lengths[points.len()];
    let mut edge = 0;
    (0..SAMPLES)
        .map(|i| {
            let distance = perimeter * i as f32 / SAMPLES as f32;
            while edge + 1 < points.len() && lengths[edge + 1] < distance {
                edge += 1;
            }
            let t = (distance - lengths[edge]) / (lengths[edge + 1] - lengths[edge]);
            lerp(points[edge], points[(edge + 1) % points.len()], t)
        })
        .collect()
}

fn cookie_four() -> Vec<Point> {
    (0..SAMPLES)
        .map(|i| {
            let angle = TAU * i as f32 / SAMPLES as f32 - FRAC_PI_2;
            let radius = 0.86 + 0.14 * (4.0 * angle).cos();
            (radius * angle.cos(), radius * angle.sin())
        })
        .collect()
}

fn shapes() -> &'static [Vec<Point>; 7] {
    static SHAPES: OnceLock<[Vec<Point>; 7]> = OnceLock::new();
    SHAPES.get_or_init(|| {
        [
            polygon(10, Some(0.58), 0.28), // Soft burst
            polygon(9, Some(0.84), 0.42),  // Nine-sided cookie
            polygon(5, None, 0.18),        // Rounded pentagon
            ellipse(2.8, 0.68),            // Pill
            polygon(8, Some(0.80), 0.26),  // Sunny
            cookie_four(),                 // Four-sided cookie
            ellipse(2.0, 0.64),            // Oval
        ]
        .map(|points| {
            let radius = points
                .iter()
                .map(|&(x, y)| x.hypot(y))
                .fold(0.0_f32, f32::max);
            let normalized: Vec<_> = points
                .iter()
                .map(|&(x, y)| (x / radius, y / radius))
                .collect();
            resample(&normalized)
        })
    })
}

fn spring(t: f32) -> f32 {
    let omega = 200.0_f32.sqrt();
    let damped = omega * 0.8; // sqrt(1 - 0.6^2)
    1.0 - (-0.6 * omega * t).exp() * ((damped * t).cos() + 0.75 * (damped * t).sin())
}

/// Unit-radius outline, with continuous clockwise rotation across all cycles.
pub(super) fn outline(seconds: f32) -> Vec<Point> {
    let position = seconds.max(0.0) / INTERVAL;
    let step = position.floor();
    let index = step as usize % 7;
    // Normalize the spring endpoint so a delayed frame or sequence wrap cannot
    // introduce a jump. Rotation may overshoot; the outline itself must not.
    let progress = spring(position.fract() * INTERVAL) / spring(INTERVAL);
    let angle = FRAC_PI_2 * (step + progress) + TAU * seconds / 4.666;
    let (s, c) = angle.sin_cos();
    shapes()[index]
        .iter()
        .zip(&shapes()[(index + 1) % 7])
        .map(|(&a, &b)| {
            let p = lerp(a, b, progress.clamp(0.0, 1.0));
            (p.0 * c - p.1 * s, p.0 * s + p.1 * c)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_remain_finite_and_inside_the_canvas() {
        for frame in 0..1800 {
            for (x, y) in outline(frame as f32 / 60.0) {
                assert!(x.is_finite() && y.is_finite());
                assert!(x.hypot(y) <= 1.001);
            }
        }
    }

    #[test]
    fn morph_and_rotation_are_continuous_at_every_boundary() {
        for step in 1..29 {
            let time = step as f32 * INTERVAL;
            for (a, b) in outline(time - 0.00001).iter().zip(outline(time + 0.00001)) {
                assert!((a.0 - b.0).hypot(a.1 - b.1) < 0.002);
            }
        }
    }
}
