//! Static stress through Nightfall's study runner. With `SOLIDCRAFT_NIGHTFALL` naming `nf` the
//! cantilever is checked against beam theory; without it, only the refusals that need no solver.

use serde_json::{Value, json};

use crate::Session;

fn nightfall() -> bool {
    std::env::var_os(super::NF).is_some_and(|v| !v.is_empty())
}

/// An aluminium cantilever 200 mm long, 10 x 10 mm, along X from the origin.
fn cantilever() -> Session {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 200, "width": 10, "height": 10, "body_name": "Beam"})).unwrap();
    s.execute("material.assign", &json!({"bodies": ["Beam"], "material": "Aluminum"})).unwrap();
    s
}

fn tip_load() -> Value {
    json!({"fixed": [[0, 5, 5]], "loads": [{"faces": [[200, 5, 5]], "force": [0, 0, -10]}], "mesh_size": 3.3})
}

#[test]
fn a_cantilever_bends_as_beam_theory_says() {
    if !nightfall() {
        eprintln!("SOLIDCRAFT_NIGHTFALL is not set: skipping");
        return;
    }
    let mut s = cantilever();
    let r = s.execute("analysis.static_stress", &tip_load()).unwrap();
    // P L³ / 3 E I for 10 N at the tip of the 200 mm aluminium (69 GPa) beam.
    let (p, l, e, i): (f64, f64, f64, f64) = (10.0, 200.0, 69_000.0, 10.0f64.powi(4) / 12.0);
    let beam = p * l.powi(3) / (3.0 * e * i);
    let d = r["max_displacement_mm"].as_f64().unwrap();
    assert!((d - beam).abs() < 0.02 * beam, "{d} vs {beam}");
    // The reactions balance the load.
    for c in 0..3 {
        let sum = r["applied_n"][c].as_f64().unwrap() + r["reaction_n"][c].as_f64().unwrap();
        assert!(sum.abs() < 1e-6, "component {c}: {sum}");
    }
    // Bending stress M c / I = 12 MPa at the root, raised at the clamped corners; aluminium yields
    // at 276 MPa: a safety factor well above one.
    let stress = r["max_von_mises_mpa"].as_f64().unwrap();
    assert!(stress > 12.0 && stress < 30.0, "{stress}");
    let sf = r["safety_factor"].as_f64().unwrap();
    assert!((sf - 276.0 / stress).abs() < 1e-9, "{sf}");
    assert_eq!(r["material"]["name"], "Aluminum");
}

#[test]
fn a_point_off_every_face_is_named_by_its_place() {
    if !nightfall() {
        return;
    }
    let mut s = cantilever();
    let mut p = tip_load();
    p["loads"][0]["faces"] = json!([[200, 50, 5]]);
    let e = s.execute("analysis.static_stress", &p).unwrap_err().to_string();
    assert!(e.contains("loads[0].faces[0]") && e.contains("no face"), "{e}");
}

#[test]
fn refusals_need_no_solver() {
    let mut s = cantilever();
    let mut cases = vec![
        (json!({"fixed": [[0, 5, 5]], "loads": [{"faces": [[200, 5, 5]]}]}), "force"),
        (json!({"fixed": [], "loads": [{"faces": [[200, 5, 5]], "force": [0, 0, -1]}]}), "fixed"),
        (json!({"fixed": [[0, 5, 5]], "loads": [{"faces": [[200, 5, 5]], "force": [0, 0, -1]}], "material": "Oak"}), "elastic"),
        (json!({"fixed": [[0, 5, 5]], "loads": [{"faces": [[200, 5, 5]], "force": [0, 0, -1]}], "mesh_size": -1}), "mesh_size"),
    ];
    if !nightfall() {
        // Without the solver every call says how to get it.
        cases = vec![(tip_load(), "SOLIDCRAFT_NIGHTFALL")];
    }
    for (p, words) in cases {
        let e = s.execute("analysis.static_stress", &p).unwrap_err().to_string();
        assert!(e.contains(words), "{p}: {e}");
    }
}

#[test]
fn a_mesh_coarser_than_the_walls_is_warned_about() {
    if !nightfall() {
        return;
    }
    // A square tube 20 x 20 x 2 mm, 100 mm long: walls 2 mm thick, meshed at 10 mm on request.
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 100, "width": 20, "height": 20, "corner": [0, -10, -10], "body_name": "Tube"})).unwrap();
    s.execute("solid.box", &json!({"length": 100, "width": 16, "height": 16, "corner": [0, -8, -8], "operation": "cut"})).unwrap();
    s.execute("material.assign", &json!({"bodies": ["Tube"], "material": "Steel"})).unwrap();
    let p = json!({"fixed": [[0, 0, 9]], "loads": [{"faces": [[100, 0, 9]], "force": [0, 0, -100]}], "mesh_size": 10});
    let r = s.execute("analysis.static_stress", &p).unwrap();
    let warning = r["warning"].as_str().unwrap_or("");
    assert!(warning.contains("coarse") && warning.contains("mm thick"), "{r}");
}
