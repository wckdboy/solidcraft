//! The Nightfall backend (docs/nightfall-backend.md): booleans, fillets and chamfers on
//! Nightfall's OpenCASCADE kernel, through `nf kernel serve` (Nightfall RFC 0015) in a child
//! process. Set `SOLIDCRAFT_NIGHTFALL` to the `nf` executable to turn it on.
//!
//! Operands go as STEP with a point on each face and edge; the result comes back as STEP with
//! each face's history in our own face and edge numbers, and becomes a [`Body`] with an exact
//! [`History`]. Nothing here links Nightfall or OpenCASCADE: a failed or refused request leaves
//! the operation to truck, so the backend can only add operations that succeed.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use crate::body::Body;
use crate::history::{Change, EdgeOrigin, EdgeProv, FaceOrigin, FaceProv, History, Side, edge_faces};
use crate::ops::BoolOp;
use crate::{KernelError, Result};

/// The environment variable naming the `nf` executable.
pub const ENV: &str = "SOLIDCRAFT_NIGHTFALL";
/// The protocol major version this client speaks.
const PROTOCOL: i64 = 1;
/// Tessellation tolerance for sample points, as a fraction of the body's size.
const SAMPLE_TOL: f64 = 2e-4;

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Drop for Server {
    fn drop(&mut self) {
        // Closing stdin ends the server; reap it so no zombie is left behind.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);
/// Set when the server could not be started: the backend stays off for this process.
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);

fn failed(what: impl std::fmt::Display) -> KernelError {
    KernelError::Failed(format!("nightfall: {what}"))
}

/// The backend is configured and has not failed to start.
pub fn enabled() -> bool {
    std::env::var_os(ENV).is_some_and(|v| !v.is_empty()) && !UNAVAILABLE.load(Ordering::Relaxed)
}

/// Seconds one request may run before the server gives it up (`SOLIDCRAFT_NIGHTFALL_TIMEOUT`,
/// default 10): truck then takes the operation, so a hang costs this long at most.
fn timeout() -> f64 {
    std::env::var("SOLIDCRAFT_NIGHTFALL_TIMEOUT").ok().and_then(|v| v.parse::<f64>().ok()).filter(|t| *t > 0.0 && t.is_finite()).unwrap_or(10.0)
}

fn start() -> Result<Server> {
    let path = std::env::var_os(ENV).ok_or_else(|| failed("SOLIDCRAFT_NIGHTFALL is not set"))?;
    let mut child = Command::new(path)
        .args(["kernel", "serve", "--timeout", &timeout().to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| failed(format!("cannot start nf kernel serve: {e}")))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err(failed("nf kernel serve has no pipes"));
    };
    let mut server = Server { child, stdin, stdout: BufReader::new(stdout), next: 1 };
    let hello = exchange(&mut server, json!({"op": "hello"}))?;
    match hello.get("protocol").and_then(Value::as_i64) {
        Some(PROTOCOL) => {
            log::info!("nightfall: {}", hello.get("kernel").and_then(Value::as_str).unwrap_or("kernel"));
            Ok(server)
        }
        other => Err(failed(format!("unsupported protocol {other:?}"))),
    }
}

fn exchange(server: &mut Server, mut request: Value) -> Result<Value> {
    let id = server.next;
    server.next += 1;
    if let Some(o) = request.as_object_mut() {
        o.insert("id".into(), json!(id));
    }
    let mut line = serde_json::to_string(&request).map_err(failed)?;
    line.push('\n');
    // SOLIDCRAFT_NIGHTFALL_DUMP=DIR keeps each request, to replay against `nf kernel serve`.
    if let Some(dir) = std::env::var_os("SOLIDCRAFT_NIGHTFALL_DUMP") {
        let path = std::path::Path::new(&dir).join(format!("request-{}-{id}.jsonl", std::process::id()));
        if let Err(e) = std::fs::write(&path, &line) {
            log::warn!("nightfall: cannot keep {}: {e}", path.display());
        }
    }
    server.stdin.write_all(line.as_bytes()).and_then(|()| server.stdin.flush()).map_err(failed)?;
    let mut reply = String::new();
    if server.stdout.read_line(&mut reply).map_err(failed)? == 0 {
        return Err(failed("nf kernel serve closed its output"));
    }
    let reply: Value = serde_json::from_str(&reply).map_err(failed)?;
    log::debug!(
        "nightfall {}: {:.1} ms in the server, {} bytes sent",
        request.get("op").and_then(Value::as_str).unwrap_or("?"),
        reply.get("ms").and_then(Value::as_f64).unwrap_or(f64::NAN),
        line.len()
    );
    if reply.get("id").and_then(Value::as_u64) != Some(id) {
        return Err(failed("reply out of order"));
    }
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        let e = reply.get("error");
        let code = e.and_then(|e| e.get("code")).and_then(Value::as_str).unwrap_or("error");
        let message = e.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("");
        return Err(KernelError::Failed(format!("nightfall {code}: {message}")));
    }
    Ok(reply)
}

/// One request to the server, started on first use. A broken pipe drops the server so the
/// next request starts a fresh one; a refusal keeps it.
fn call(request: Value) -> Result<Value> {
    let mut guard = SERVER.lock().map_err(|_| failed("server lock poisoned"))?;
    if guard.is_none() {
        match start() {
            Ok(s) => *guard = Some(s),
            Err(e) => {
                UNAVAILABLE.store(true, Ordering::Relaxed);
                return Err(e);
            }
        }
    }
    let server = guard.as_mut().ok_or_else(|| failed("no server"))?;
    let reply = exchange(server, request);
    if let Err(KernelError::Failed(m)) = &reply
        && m.starts_with("nightfall: ")
    {
        *guard = None;
    }
    reply
}

fn xyz(p: Vec3) -> Value {
    json!([p.x, p.y, p.z])
}

/// A point inside each face, clear of its boundary: the incenter of the face's display triangle
/// with the largest inscribed circle (the face's boundary is made of triangle sides, so no part
/// of it is nearer than that circle's radius).
fn face_samples(body: &Body, tol: f64) -> Result<Vec<Value>> {
    let m = body.tessellate(tol)?;
    let mut best: Vec<Option<(f64, Vec3)>> = vec![None; body.face_count()];
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        let Some([a, b, c]) = m.tri(t) else { continue };
        let (la, lb, lc) = ((b - c).len(), (c - a).len(), (a - b).len());
        let perimeter = la + lb + lc;
        if !(perimeter > 0.0) {
            continue;
        }
        let inradius = (b - a).cross(c - a).len() / perimeter;
        let incenter = (a * la + b * lb + c * lc) / perimeter;
        if let Some(slot) = best.get_mut(*f as usize)
            && slot.is_none_or(|(r, _)| inradius > r)
        {
            *slot = Some((inradius, incenter));
        }
    }
    best.into_iter()
        .enumerate()
        .map(|(i, s)| match s {
            Some((r, p)) if r > 4.0 * tol => Ok(xyz(p)),
            _ => Err(failed(format!("face {i} is too narrow to sample"))),
        })
        .collect()
}

fn operand(body: &Body) -> Result<(Value, f64)> {
    body.require_brep("the Nightfall backend")?;
    let tol = body.size() * SAMPLE_TOL;
    let step = crate::step_export(&[body], "SolidCraft")?;
    let edges: Vec<Value> = body.edges(tol)?.iter().map(|e| xyz(e.mid)).collect();
    Ok((json!({"step": step, "faces": face_samples(body, tol)?, "edges": edges}), tol))
}

fn origin(v: &Value) -> Result<FaceOrigin> {
    let n = |k: &str| v.get(k).and_then(Value::as_u64).map(|x| x as usize);
    match (n("operand"), n("face"), n("edge")) {
        (Some(operand), Some(face), None) => Ok(FaceOrigin::Face { operand, face }),
        (Some(operand), None, Some(edge)) => Ok(FaceOrigin::Edge { operand, edge }),
        _ => Err(failed("malformed face origin")),
    }
}

fn change(v: &Value) -> Result<Change> {
    match v.as_str() {
        Some("kept") => Ok(Change::Kept),
        Some("modified") => Ok(Change::Modified),
        Some("split") => Ok(Change::Split),
        Some("merged") => Ok(Change::Merged),
        Some("generated") => Ok(Change::Generated),
        _ => Err(failed("unknown face change")),
    }
}

/// The solid of a result's STEP, with the STEP face each of its faces was read from. Several
/// solids (a union of bodies apart, a cut in two) are one body of several pieces, as truck
/// makes them, its faces followed back to the pieces' through `join_apart`'s history.
fn read(step: &str) -> Result<(Body, Vec<u64>)> {
    let import = crate::step_import(step)?;
    if import.bodies.is_empty() || import.bodies.iter().any(|b| !b.closed) {
        return Err(failed("the result is not closed solids"));
    }
    if let [one] = import.bodies.as_slice() {
        return Ok((one.body.clone(), one.face_origin.clone()));
    }
    let pieces: Vec<Body> = import.bodies.iter().map(|b| b.body.clone()).collect();
    let joined = crate::join_apart(&pieces)?.ok_or_else(|| failed("cannot join the result's pieces"))?;
    let history = joined.history().ok_or_else(|| failed("the joined pieces have no history"))?;
    let origin = history
        .faces
        .iter()
        .map(|f| match f.origins.as_slice() {
            [FaceOrigin::Face { operand, face }] => import.bodies.get(*operand).and_then(|b| b.face_origin.get(*face)).copied(),
            _ => None,
        })
        .collect::<Option<Vec<u64>>>()
        .ok_or_else(|| failed("a joined face has no single piece face"))?;
    Ok((joined.without_history(), origin))
}

/// The result of a reply: its solid, numbered as the reply numbers it (STEP face order, which
/// `face_origin` records for each of our faces), with the history the reply gives.
fn result(reply: &Value, operands: &[&Body]) -> Result<Body> {
    let step = reply.get("step").and_then(Value::as_str).ok_or_else(|| failed("reply has no STEP"))?;
    let entries = reply.get("faces").and_then(Value::as_array).ok_or_else(|| failed("reply has no faces"))?;
    let (body, face_origin) = read(step)?;
    let mut files: Vec<u64> = face_origin.clone();
    files.sort_unstable();
    files.dedup();
    if files.len() != entries.len() || face_origin.len() != body.face_count() {
        return Err(failed("result faces do not match the reply"));
    }
    let rank = |id: &u64| files.binary_search(id).map_err(|_| failed("unknown result face"));
    // The reply's areas confirm the numbering: a face read in another order would not match.
    // Display triangles cut curved faces short by their sag; 1% holds well within that.
    let tol = body.size() * SAMPLE_TOL;
    let mut areas = vec![0.0; files.len()];
    for (f, id) in body.faces(tol)?.iter().zip(&face_origin) {
        if let Some(a) = areas.get_mut(rank(id)?) {
            *a += f.area;
        }
    }
    for (k, (area, entry)) in areas.iter().zip(entries).enumerate() {
        let expected = entry.get("area").and_then(Value::as_f64).unwrap_or(f64::NAN);
        if !((area - expected).abs() <= 1e-2 * expected.max(tol * tol)) {
            return Err(failed(format!("result face {k} has area {area}, the reply {expected}")));
        }
    }
    let mut faces = Vec::with_capacity(body.face_count());
    for id in &face_origin {
        let k = rank(id)?;
        let entry = entries.get(k).ok_or_else(|| failed("unknown result face"))?;
        let mut origins: Vec<FaceOrigin> =
            entry.get("from").and_then(Value::as_array).map(|a| a.iter().map(origin).collect()).transpose()?.unwrap_or_default();
        // A shell's inner wall: an offset of the faces it lines, as truck's shell records it.
        for o in entry.get("offset").and_then(Value::as_array).into_iter().flatten() {
            match origin(o)? {
                FaceOrigin::Face { operand, face } => origins.push(FaceOrigin::Offset { operand, face }),
                _ => return Err(failed("malformed offset origin")),
            }
        }
        let pieces = face_origin.iter().filter(|x| *x == id).count();
        let change = if pieces > 1 && !origins.is_empty() { Change::Split } else { change(entry.get("change").unwrap_or(&Value::Null))? };
        faces.push(FaceProv { origins, change });
    }
    let edges = edge_history(&body, &faces, operands);
    let counts: Vec<(usize, usize)> = operands.iter().map(|b| (b.face_count(), b.edge_count())).collect();
    let mut history = History { operands: operands.len(), faces, edges };
    history.check_counts((body.face_count(), body.edge_count()), &counts)?;
    // OpenCASCADE writes a whole turn of a cylinder or cone as one face along a seam; truck
    // builds such walls in two halves, and its later operations (a shell's offset, a chamfer)
    // expect that. Halve them as the kernel's own boolean does, the history composed through.
    let body = match crate::seam_faces::halved(&body) {
        Some((halves, Some(mut split))) if halves.face_count() != body.face_count() => {
            split.substitute(0, &history)?;
            history = split;
            halves
        }
        _ => body,
    };
    let body = body.with_history_counts(history, &counts)?;
    // What truck's results carry from their operands: face sources through the history (a
    // face kept or modified keeps its construction identity), protection and colour.
    let sources = body.history().map(|h| h.face_sources(operands)).filter(|s| s.iter().any(Option::is_some));
    let body = body.with_face_sources(sources.map(std::sync::Arc::new)).with_color(operands.first().and_then(|o| o.color()));
    Ok(operands.iter().fold(body, |b, o| b.inherit_regularization_protection(o)))
}

/// Each result edge's origin, from its faces' origins: an operand's edge where both faces come
/// from the two faces of one operand that met along exactly one edge (or a seam of one face),
/// else where those faces newly meet. An edge with a side new to the operation has none.
fn edge_history(body: &Body, faces: &[FaceProv], operands: &[&Body]) -> Vec<EdgeProv> {
    let before: Vec<Vec<Vec<usize>>> = operands.iter().map(|b| edge_faces(&b.solid)).collect();
    let side = |f: &usize| faces.get(*f).and_then(|p| Side::new(p.origins.iter().copied()));
    edge_faces(&body.solid)
        .into_iter()
        .map(|on| {
            let sides: Vec<Option<Side>> = on.iter().map(side).collect();
            let origins = match sides.as_slice() {
                [Some(a), Some(b)] => match (a.single(), b.single()) {
                    (Some(FaceOrigin::Face { operand: p, face: x }), Some(FaceOrigin::Face { operand: q, face: y })) if p == q => {
                        let mut pair = [x, y];
                        pair.sort_unstable();
                        let shared: Vec<usize> = before
                            .get(p)
                            .into_iter()
                            .flatten()
                            .enumerate()
                            .filter(|(_, f)| {
                                let mut g = (*f).clone();
                                g.sort_unstable();
                                g == pair
                            })
                            .map(|(e, _)| e)
                            .collect();
                        match shared.as_slice() {
                            [edge] => vec![EdgeOrigin::Edge { operand: p, edge: *edge }],
                            _ => vec![EdgeOrigin::meet(a.clone(), b.clone())],
                        }
                    }
                    _ => vec![EdgeOrigin::meet(a.clone(), b.clone())],
                },
                [Some(a)] => match a.single() {
                    Some(FaceOrigin::Face { operand, face }) => {
                        let seams: Vec<usize> =
                            before.get(operand).into_iter().flatten().enumerate().filter(|(_, f)| f.as_slice() == [face]).map(|(e, _)| e).collect();
                        match seams.as_slice() {
                            [edge] => vec![EdgeOrigin::Edge { operand, edge: *edge }],
                            _ => vec![],
                        }
                    }
                    _ => vec![],
                },
                _ => vec![],
            };
            EdgeProv { faces: on, origins }
        })
        .collect()
}

fn warn_fallback(what: &str, e: &KernelError) {
    log::warn!("nightfall {what} refused, using truck: {e}");
    #[cfg(test)]
    eprintln!("nightfall {what} refused: {e}");
}

/// `a op b` on Nightfall, or `None` to leave it to truck (the backend is off, an operand is a
/// mesh, or the request failed). The history's operands are `[a, b]`, as truck's.
pub(crate) fn boolean(a: &Body, b: &Body, op: BoolOp) -> Option<Body> {
    if a.is_mesh() || b.is_mesh() {
        return None;
    }
    let kind = match op {
        BoolOp::Union => "union",
        BoolOp::Cut => "subtract",
        BoolOp::Intersect => "intersect",
    };
    let run = || -> Result<Body> {
        let (pa, ta) = operand(a)?;
        let (pb, tb) = operand(b)?;
        let reply = call(json!({"op": "boolean", "kind": kind, "operands": [pa, pb], "sample_tolerance": 2.0 * ta.max(tb)}))?;
        let body = result(&reply, &[a, b])?;
        // The planar roles truck's boolean keeps, by the same rules.
        match op {
            BoolOp::Cut => body.with_preserved_boolean_planar_sources(a, b, true),
            BoolOp::Union | BoolOp::Intersect => body.with_preserved_union_planar_sources(a, b),
        }
    };
    run().map_err(|e| warn_fallback("boolean", &e)).ok()
}

/// A constant fillet (`round`) or equal chamfer of the edges nearest `picks`, or `None` to leave
/// it to truck. New faces come from their edges, as truck's blends record them.
pub(crate) fn blend(body: &Body, picks: &[Vec3], size: f64, round: bool) -> Option<Body> {
    if body.is_mesh() || picks.is_empty() {
        return None;
    }
    let run = || -> Result<Body> {
        let (p, tol) = operand(body)?;
        let mut edges = Vec::with_capacity(picks.len());
        for pick in picks {
            let (e, _) = body.nearest_edge(*pick, tol)?.ok_or_else(|| failed("no edge near a selected point"))?;
            if !edges.contains(&e) {
                edges.push(e);
            }
        }
        let (op, key) = if round { ("fillet", "radius") } else { ("chamfer", "distance") };
        let reply = call(json!({"op": op, "operands": [p], "edges": edges, key: size, "sample_tolerance": 2.0 * tol}))?;
        result(&reply, &[body])
    };
    run().map_err(|e| warn_fallback(if round { "fillet" } else { "chamfer" }, &e)).ok()
}

/// A shell `thickness` thick inward with the faces nearest `open` removed, or `None` to leave
/// it to truck (no open face, or the request failed). Inner walls are offsets of the faces
/// they line, as truck's shell records them.
pub(crate) fn shell(body: &Body, open: &[Vec3], thickness: f64) -> Option<Body> {
    if body.is_mesh() || open.is_empty() {
        return None;
    }
    let run = || -> Result<Body> {
        let (p, tol) = operand(body)?;
        let m = body.tessellate(tol)?;
        let mut faces: Vec<usize> = Vec::new();
        for at in open {
            let near = m
                .triangles
                .iter()
                .zip(&m.tri_face)
                .filter_map(|(t, f)| m.tri(t).map(|[a, b, c]| (crate::polyhedron::point_tri(*at, a, b, c), *f as usize)))
                .min_by(|x, y| x.0.total_cmp(&y.0));
            match near {
                Some((d, f)) if d < body.size() * 1e-3 + 1e-6 => {
                    if !faces.contains(&f) {
                        faces.push(f);
                    }
                }
                _ => return Err(failed("no face near an open point")),
            }
        }
        let reply = call(json!({"op": "shell", "operands": [p], "faces": faces, "thickness": thickness, "sample_tolerance": 2.0 * tol}))?;
        result(&reply, &[body])?.with_shell_planar_sources(body, thickness, open)
    };
    run().map_err(|e| warn_fallback("shell", &e)).ok()
}

#[cfg(test)]
#[path = "nightfall_tests.rs"]
mod tests;
