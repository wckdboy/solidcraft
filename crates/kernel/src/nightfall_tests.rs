//! Run with `SOLIDCRAFT_NIGHTFALL=/path/to/nf`; without it these tests have nothing to check.

use super::*;
use crate::history::{Change, FaceOrigin};
use crate::{box_solid, cylinder, measure};

fn on() -> bool {
    let on = enabled();
    if !on {
        eprintln!("{ENV} is not set: skipping");
    }
    on
}

fn block() -> Body {
    box_solid(Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 30.0, 10.0)).unwrap()
}

#[test]
fn cut_carries_exact_history() {
    if !on() {
        return;
    }
    let a = block();
    let b = cylinder(Vec3::new(20.0, 15.0, -5.0), Vec3::new(0.0, 0.0, 1.0), 6.0, 20.0).unwrap();
    let r = boolean(&a, &b, BoolOp::Cut).unwrap();
    let m = measure(&r).unwrap();
    let exact = 12000.0 - 360.0 * std::f64::consts::PI;
    assert!((m.volume - exact).abs() < 1e-4 * exact, "volume {}", m.volume);
    let h = r.history().unwrap();
    assert_eq!(h.operands, 2);
    let kept = h.faces.iter().filter(|f| f.change == Change::Kept).count();
    assert_eq!(kept, 4, "{:?}", h.faces);
    // The bore comes from both of truck's half-cylinder faces.
    let bore: Vec<_> = h.faces.iter().filter(|f| f.origins.iter().any(|o| matches!(o, FaceOrigin::Face { operand: 1, .. }))).collect();
    assert_eq!(bore.len(), 1);
    assert_eq!(bore[0].origins, vec![FaceOrigin::Face { operand: 1, face: 1 }, FaceOrigin::Face { operand: 1, face: 2 }]);
    // Every edge of the block's sides survives as one of its own edges.
    assert!(h.edges.iter().filter(|e| matches!(e.origins.as_slice(), [EdgeOrigin::Edge { operand: 0, .. }])).count() >= 8);
}

#[test]
fn fillet_faces_come_from_their_edges() {
    if !on() {
        return;
    }
    let a = block();
    let pick = Vec3::new(20.0, 0.0, 10.0);
    let edge = a.nearest_edge(pick, 1e-3).unwrap().unwrap().0;
    let r = blend(&a, &[pick], 2.0, true).unwrap();
    let m = measure(&r).unwrap();
    let exact = 12000.0 - (4.0 - std::f64::consts::PI) * 40.0;
    assert!((m.volume - exact).abs() < 1e-4 * exact, "volume {}", m.volume);
    let h = r.history().unwrap();
    let made: Vec<_> = h.faces.iter().filter(|f| f.change == Change::Generated).collect();
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].origins, vec![FaceOrigin::Edge { operand: 0, edge }]);
}
