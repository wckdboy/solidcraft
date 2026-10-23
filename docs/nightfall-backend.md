# Nightfall backend

This fork of SolidCraft can run its hardest kernel operations on
[Nightfall](https://github.com/wckdboy/nightfall)'s kernel: OpenCASCADE 7.9.3 with exact face and
edge history. SolidCraft keeps everything else, including its document, timeline, naming, engine,
MCP server, UI and truck. The aim is SolidCraft's AI-native modelling on an industrial kernel,
without linking that kernel.

## Turning it on

Build Nightfall, then point SolidCraft at its `nf` executable:

```sh
export SOLIDCRAFT_NIGHTFALL=/path/to/nightfall/build/occt/tools/cli/nf
cargo run --release -p solidcraft        # or solidcraft-cli, or the MCP server
```

With the variable unset, nothing changes, and the browser build never has the backend.

## What runs where

Only the operations a user's features make are routed. In the kernel these are
`feature_boolean`, `feature_fillet`, `feature_chamfer` and `feature_shell`; the document calls them
for joins, cuts, intersects, fillets, chamfers and inward shells. The kernel's own algorithms (blends, shells, coplanar
joins and so on) keep calling truck's `boolean`, `fillet` and `chamfer`, so the intermediate
bodies they were written for are unchanged.

| Feature operation | With the backend | Without |
| --- | --- | --- |
| Join, cut, intersect | Nightfall first, truck if refused | truck |
| Fillet (constant radius) | Nightfall first, truck if refused | truck |
| Chamfer (equal distance) | Nightfall first, truck if refused | truck |
| Shell (inward, with open faces) | Nightfall first, truck if refused | truck |
| Everything else | truck | truck |

A refused request falls back to truck and logs `nightfall … refused, using truck: …` at warn
level. A request can be refused because:

- an operand is a mesh;
- a face is too narrow to sample;
- OpenCASCADE fails the operation;
- the result does not read back as one closed solid.

The backend can therefore only add operations that succeed. It never removes one.

## How it works

`nf kernel serve` is a child process started on first use. It speaks Nightfall RFC 0015: one
JSON object per line on stdin and stdout.

1. **Operands.** Each body goes as STEP text (`step_export`), together with one point inside each
   face and one point on each edge, in SolidCraft's own face and edge order.
   - Each face point is the incenter of the face's display triangle with the largest inscribed
     circle, so no part of the face's boundary is nearer than that circle's radius.
   - Nightfall maps every point to its own face or edge and refuses an ambiguous match.
   - Points are needed because the STEP exporter merges a split face's pieces, so face order
     does not survive the trip to Nightfall.
2. **Result.** Nightfall returns the result as STEP and, for each result face, its change (`kept`,
   `modified`, `split`, `merged` or `generated`), its origins in SolidCraft's numbering, its role
   (`fillet`, `chamfer`) and its area.
3. **Reading back.** `step_import` reads the result. `face_origin` gives each of our faces its
   STEP face, and the STEP face's rank is its number in the reply. The reply's areas are checked
   against ours (within 1%, the tessellation's sag), so a misnumbered face is refused, never used.
4. **Shape.** OpenCASCADE writes a whole turn of a cylinder or cone as one face running along a
   seam. Truck builds such walls in two halves, and its later operations expect that, so the
   result's seamed faces are cut free of their seams (`seam_faces::halved`). That history is
   composed with the reply's. A result in several solids becomes one body of several pieces
   (`join_apart`). Results carry face sources, planar roles, protection and colour as truck's
   boolean does.
5. **History.** Face history comes from the reply. Edge history follows from it:
   - Where both faces of a result edge come from one operand's two faces, and those faces met
     along exactly one edge, the result edge is that edge.
   - Otherwise it is where those faces newly meet (`EdgeOrigin::Meet`).
   - The body carries the result as an exact `History`, so persistent names, face attachments
     and lineage work as they do with truck.

## Settings

| Variable | Meaning |
| --- | --- |
| `SOLIDCRAFT_NIGHTFALL` | Path of `nf`; unset turns the backend off |
| `SOLIDCRAFT_NIGHTFALL_TIMEOUT` | Seconds one request may run before truck takes it (10) |
| `SOLIDCRAFT_NIGHTFALL_DUMP` | Folder that keeps every request, to replay with `nf kernel serve < request.jsonl` |

While the backend is on, the app bar shows a **Nightfall OCCT** badge.

## Testing

```sh
SOLIDCRAFT_NIGHTFALL=/path/to/nf cargo test -p solidcraft-kernel --lib nightfall
```

Without the variable, these tests skip themselves. Running the whole kernel suite with the
variable set shows how much of SolidCraft's behaviour holds with OpenCASCADE underneath. Tests
that pin truck's exact face order or its fallbacks are expected to differ.

## Limits

- The backend handles four operations. Variable fillets, G2 blends, outward shells, drafts and offsets
  stay on truck.
- Each call ships its operands as STEP, which costs a few milliseconds for small parts. A later
  protocol version can cache operands by digest.
- Requests are serialized through one server process.
