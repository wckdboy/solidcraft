//! Static stress analysis (`analysis.static_stress`): linear elastic finite elements of one body,
//! solved by Nightfall's study runner (`nf study solve`: Netgen meshes, MFEM solves with
//! second-order tetrahedra) in a separate process, as the FreeCAD oracle runs FreeCAD: nothing of
//! either is linked. The body goes as STEP, its supports and loads as points on its faces; the
//! answer is the largest von Mises stress and displacement, the reactions that balance the loads
//! and, for a material with a known yield strength, the safety factor. Desktop only: set
//! `SOLIDCRAFT_NIGHTFALL` to Nightfall's `nf`.

use serde_json::{Value, json};

use super::CommandSpec;
use crate::params::bad;
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("analysis.static_stress", "Static Stress", static_stress).icon("compute").noundo().params(
    "body?: name (default: the only body); fixed: [[x,y,z]…] a point inside each face held in place (body_topology gives each face's point); \
     loads: [{faces: [[x,y,z]…], force?: [fx,fy,fz] N (the total, spread over the faces by area), pressure?: MPa (into the faces)}]; \
     material?: a built-in material (default: the body's; Oak and the default material have no elastic data); youngs_modulus_mpa?, \
     poisson_ratio?, yield_mpa?: override the material's; mesh_size?: mm (default: a fifteenth of the body's size or three quarters of its walls' thickness, whichever is smaller, at least 0.5; a coarser one is answered with a warning); refine?: \
     true (three meshes, each finer by 1.5: the convergence order of each result); budget_s?: seconds (default 600) → max_von_mises_mpa, \
     max_displacement_mm, yield_mpa and safety_factor (yield / max_von_mises) when the yield is known, applied_n and reaction_n (they \
     balance), material {name, youngs_modulus_mpa, poisson_ratio}, mesh {size_mm, nodes, tetrahedra}, runs (every mesh), convergence_order?, \
     solver, mesher. Linear and small-strain: past yield the stress is not the part's. Needs SOLIDCRAFT_NIGHTFALL (Nightfall's nf), desktop only",
)];

/// Typical room-temperature properties of the built-in materials (public handbook values):
/// Young's modulus and yield strength in MPa, Poisson's ratio. Wood is left out: its stiffness
/// depends on the grain direction. Glass and cast iron are brittle: no yield strength.
const ELASTIC: [(&str, f64, f64, Option<f64>); 12] = [
    ("Steel", 200_000.0, 0.30, Some(250.0)),
    ("Stainless Steel", 193_000.0, 0.29, Some(215.0)),
    ("Aluminum", 69_000.0, 0.33, Some(276.0)),
    ("Brass", 100_000.0, 0.34, Some(200.0)),
    ("Copper", 117_000.0, 0.34, Some(70.0)),
    ("Titanium", 114_000.0, 0.34, Some(880.0)),
    ("Cast Iron", 110_000.0, 0.26, None),
    ("ABS Plastic", 2_200.0, 0.35, Some(40.0)),
    ("PLA", 3_500.0, 0.36, Some(50.0)),
    ("Nylon", 2_700.0, 0.39, Some(70.0)),
    ("Polycarbonate", 2_400.0, 0.37, Some(62.0)),
    ("Glass", 70_000.0, 0.22, None),
];

/// The environment variable naming Nightfall's `nf`.
const NF: &str = "SOLIDCRAFT_NIGHTFALL";
/// Most support and load points one call takes.
const MAX_POINTS: usize = 1000;

fn static_stress(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "analysis.static_stress";
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err(bad(cmd, "not supported yet: analysis runs Nightfall's solver, in the desktop app only"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    native::run(cmd, s, p)
}

/// Face points: `[[x, y, z], …]`, finite, at least one.
fn points(cmd: &str, v: Option<&Value>, what: &str) -> Result<Vec<[f64; 3]>> {
    let list = v.and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("`{what}` must be [[x,y,z], …] points on faces")))?;
    if list.is_empty() || list.len() > MAX_POINTS {
        return Err(bad(cmd, format!("`{what}` takes 1…{MAX_POINTS} points")));
    }
    list.iter()
        .map(|q| match crate::params::vec3(q) {
            Some(v) if v.is_finite() => Ok([v.x, v.y, v.z]),
            _ => Err(bad(cmd, format!("`{what}`: a point is three finite numbers in mm"))),
        })
        .collect()
}

/// The study object `nf study solve` takes: supports and loads by face points.
fn study_spec(cmd: &str, p: &Value, e: f64, nu: f64, size: f64) -> Result<Value> {
    let mut boundaries = vec![json!({"type": "fixed", "faces": {"points": points(cmd, p.get("fixed"), "fixed")?}})];
    let loads = p.get("loads").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`loads` must be a list of {faces, force} or {faces, pressure}"))?;
    if loads.is_empty() || loads.len() > MAX_POINTS {
        return Err(bad(cmd, format!("`loads` takes 1…{MAX_POINTS} loads")));
    }
    for (i, load) in loads.iter().enumerate() {
        let faces = json!({"points": points(cmd, load.get("faces"), &format!("loads[{i}].faces"))?});
        match (load.get("force"), load.get("pressure").and_then(Value::as_f64)) {
            (Some(f), None) => {
                let f =
                    crate::params::vec3(f).filter(|v| v.is_finite()).ok_or_else(|| bad(cmd, format!("`loads[{i}].force` must be [fx, fy, fz] N")))?;
                boundaries
                    .push(json!({"type": "force", "faces": faces, "value": [format!("{} N", f.x), format!("{} N", f.y), format!("{} N", f.z)]}));
            }
            (None, Some(mpa)) if mpa.is_finite() => boundaries.push(json!({"type": "pressure", "faces": faces, "value": format!("{mpa} MPa")})),
            _ => return Err(bad(cmd, format!("`loads[{i}]` needs a force [fx, fy, fz] (N) or a pressure (MPa), one of them"))),
        }
    }
    Ok(json!({
        "kind": "static_structural",
        "material": {"youngs_modulus": format!("{e} MPa"), "poisson_ratio": nu},
        "boundaries": boundaries,
        "mesh": {"size": format!("{size} mm")},
    }))
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::Read;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use super::{ELASTIC, NF, study_spec};
    use crate::params::{bad, bool_, num, str_};
    use crate::{Result, Session};

    pub(super) fn run(cmd: &str, s: &mut Session, p: &Value) -> Result<Value> {
        let nf = std::env::var_os(NF)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| bad(cmd, "not available: set SOLIDCRAFT_NIGHTFALL to Nightfall's nf (it solves the analysis)"))?;
        let st = s.world_state();
        let body = match str_(p, "body") {
            Some(name) => st.body(name).ok_or_else(|| bad(cmd, format!("no body named {name}")))?,
            None => match st.bodies.as_slice() {
                [one] => one,
                _ => return Err(bad(cmd, "`body` is required when the design has more than one body")),
            },
        };
        if body.body.is_mesh() {
            return Err(bad(cmd, "a mesh body (STL, 3MF, OBJ) has no solid faces to analyse"));
        }
        let material = str_(p, "material")
            .map(str::to_string)
            .unwrap_or_else(|| s.doc.materials.get(body.definition_name()).cloned().unwrap_or_else(|| "Default".into()));
        let known = ELASTIC.iter().find(|(n, ..)| n.eq_ignore_ascii_case(material.trim()));
        let e = num(p, "youngs_modulus_mpa").or(known.map(|k| k.1));
        let nu = num(p, "poisson_ratio").or(known.map(|k| k.2));
        let yield_mpa = num(p, "yield_mpa").or(known.and_then(|k| k.3));
        let (Some(e), Some(nu)) = (e, nu) else {
            return Err(bad(
                cmd,
                format!("{material} has no elastic data: give youngs_modulus_mpa and poisson_ratio, or a material such as Steel or Aluminum"),
            ));
        };
        if !(e > 0.0 && e.is_finite() && nu > -1.0 && nu < 0.5) {
            return Err(bad(cmd, "youngs_modulus_mpa must be positive and poisson_ratio between -1 and 0.5"));
        }
        // A wall's thickness, roughly: twice the volume over the area for a thin-walled part. A mesh
        // coarser than that bends a wall with one element through it, too stiff: the stress comes
        // out low, the dangerous way.
        let wall = solidcraft_kernel::measure(&body.body).ok().map(|m| 2.0 * m.volume / m.area).filter(|t| *t > 0.0 && t.is_finite());
        let fine = wall.map(|t| 0.75 * t).unwrap_or(f64::INFINITY);
        let size = match num(p, "mesh_size") {
            Some(h) if h > 0.0 && h.is_finite() => h,
            Some(_) => return Err(bad(cmd, "`mesh_size` must be a positive length in mm")),
            None => (body.body.size() / 15.0).min(fine).max(0.5),
        };
        let coarse = size > 1.5 * fine;
        let spec = study_spec(cmd, p, e, nu, size)?;
        let budget = Duration::from_secs_f64(num(p, "budget_s").filter(|b| *b > 0.0 && b.is_finite()).unwrap_or(600.0).min(86_400.0));
        let step = solidcraft_kernel::step_export(&[&body.body], "SolidCraft").map_err(|e| bad(cmd, format!("STEP export: {e}")))?;

        let dir = scratch(cmd)?;
        let result = solve(cmd, &nf, &dir, &step, &spec, size, bool_(p, "refine").unwrap_or(false), budget);
        let _ = std::fs::remove_dir_all(&dir);
        let out = result?;

        let q = &out["quantities"];
        let (stress, displacement) = (q["max_von_mises"].as_f64().unwrap_or(f64::NAN), q["max_displacement"].as_f64().unwrap_or(f64::NAN));
        let last = out["runs"].as_array().and_then(|r| r.last()).cloned().unwrap_or(Value::Null);
        let mut reply = json!({
            "body": body.name,
            "max_von_mises_mpa": stress,
            "max_displacement_mm": displacement,
            "applied_n": last["applied_force"],
            "reaction_n": last["reaction_force"],
            "material": {"name": material, "youngs_modulus_mpa": e, "poisson_ratio": nu},
            "mesh": {"size_mm": last["mesh_size"], "nodes": last["nodes"], "tetrahedra": last["tetrahedra"]},
            "runs": out["runs"],
            "solver": out["provenance"]["solver"],
            "mesher": out["provenance"]["mesher"],
        });
        if let Some(y) = yield_mpa {
            reply["yield_mpa"] = json!(y);
            if stress > 0.0 {
                reply["safety_factor"] = json!(y / stress);
            }
        }
        if coarse {
            reply["warning"] = json!(format!(
                "the mesh ({size:.3} mm) is coarse for walls about {:.3} mm thick: the stress may read low; use a smaller mesh_size or refine",
                fine / 0.75
            ));
        }
        if let Some(order) = out.get("convergence_order") {
            reply["convergence_order"] = order.clone();
        }
        Ok(reply)
    }

    /// A new folder of this run's own in the temporary folder. Created exclusively: a name already
    /// there (left over, or planted) is never written into; another name is tried.
    fn scratch(cmd: &str) -> Result<std::path::PathBuf> {
        static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        for _ in 0..16 {
            let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("solidcraft-analysis-{}-{nanos}-{run}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(bad(cmd, format!("temporary folder: {e}"))),
            }
        }
        Err(bad(cmd, "temporary folder: no free name"))
    }

    /// Writes the part and the study, runs `nf study solve` within `budget` and reads its answer.
    #[allow(clippy::too_many_arguments)]
    fn solve(
        cmd: &str,
        nf: &std::ffi::OsStr,
        dir: &std::path::Path,
        step: &str,
        spec: &Value,
        size: f64,
        refine: bool,
        budget: Duration,
    ) -> Result<Value> {
        let (part, study) = (dir.join("part.step"), dir.join("study.json"));
        std::fs::write(&part, step)
            .and_then(|()| std::fs::write(&study, spec.to_string()))
            .map_err(|e| bad(cmd, format!("writing the study: {e}")))?;
        let mut command = std::process::Command::new(nf);
        command.arg("study").arg("solve").arg(&part).arg(&study);
        if refine {
            command.arg("--refine").arg(format!("{},{},{}", size, size / 1.5, size / 2.25));
        }
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| bad(cmd, format!("cannot start Nightfall's nf: {e}")))?;
        // Read the answer as it comes, so a long one never fills the pipe and stalls the solver.
        let mut stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut text = String::new();
            let read = stdout.as_mut().map(|o| o.read_to_string(&mut text));
            (text, read)
        });
        let started = Instant::now();
        let waited = loop {
            match child.try_wait() {
                Ok(Some(_)) => break Ok(()),
                Ok(None) if started.elapsed() > budget => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(bad(cmd, format!("the analysis ran past its budget of {} s (budget_s)", budget.as_secs())));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(bad(cmd, format!("waiting for the solver: {e}")));
                }
            }
        };
        let (text, read) = reader.join().map_err(|_| bad(cmd, "reading the solver failed"))?;
        waited?;
        if let Some(Err(e)) = read {
            return Err(bad(cmd, format!("reading the solver: {e}")));
        }
        let out: Value = serde_json::from_str(text.trim()).map_err(|_| bad(cmd, "the solver gave no answer"))?;
        if let Some(errors) = out.get("errors").and_then(Value::as_array) {
            let why: Vec<String> = errors
                .iter()
                .map(|e| {
                    let at = e["pointer"].as_str().unwrap_or("");
                    let message = e["message"].as_str().unwrap_or("");
                    // A point that finds no face, or several, is named by its place in the call.
                    match at.strip_prefix("/boundaries/") {
                        Some(rest) => format!("{message} ({})", which(rest)),
                        None => message.to_string(),
                    }
                })
                .collect();
            return Err(bad(cmd, why.join("; ")));
        }
        Ok(out)
    }

    /// `0/faces/points/2` → `fixed[2]`, `3/faces/points/0` → `loads[2].faces[0]` (the study's
    /// boundaries are the support, then each load).
    fn which(rest: &str) -> String {
        let parts: Vec<&str> = rest.split('/').collect();
        let index = |s: Option<&&str>| s.and_then(|s| s.parse::<usize>().ok());
        match (index(parts.first()), index(parts.get(3))) {
            (Some(0), Some(k)) => format!("fixed[{k}]"),
            (Some(b), Some(k)) => format!("loads[{}].faces[{k}]", b - 1),
            (Some(0), None) => "fixed".into(),
            (Some(b), None) => format!("loads[{}]", b - 1),
            _ => rest.to_string(),
        }
    }
}

#[cfg(test)]
#[path = "analysis_tests.rs"]
mod tests;
