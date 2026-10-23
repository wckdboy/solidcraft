# Static stress analysis

`analysis.static_stress` tells you whether a part holds its load: the largest stress and
deflection, the reactions at the supports, and the safety factor against the material's yield.
It is linear elastic finite element analysis with second-order tetrahedra. Nightfall's study
runner meshes the part with Netgen and solves with MFEM, in a separate program, the way the
FreeCAD oracle runs FreeCAD: nothing of either is linked into SolidCraft.

## Setting it up

The analysis runs Nightfall's `nf`, built with its studies. Point SolidCraft at it:

```sh
export SOLIDCRAFT_NIGHTFALL=/path/to/nightfall/build/occt/tools/cli/nf
```

Without it, the command says so and does nothing. The browser build has no analysis.

## Running it

Supports and loads are points inside the faces they act on. `body_topology` (MCP) lists every
face with a `point` that lies on it.

```json
{"command": "analysis.static_stress", "params": {
  "body": "Bracket",
  "fixed": [[0, 5, 5]],
  "loads": [{"faces": [[200, 5, 5]], "force": [0, 0, -10]}]
}}
```

| Parameter | Meaning |
| --- | --- |
| `fixed` | Points inside the faces held in place |
| `loads` | Each `{faces, force: [fx, fy, fz]}` in N (the total, spread by area), or `{faces, pressure}` in MPa |
| `material` | A built-in material; default: the body's |
| `youngs_modulus_mpa`, `poisson_ratio`, `yield_mpa` | Override the material's values |
| `mesh_size` | Element size in mm; default: a fifteenth of the part's size |
| `refine` | `true`: three meshes, each 1.5 times finer, and the convergence order of each result |
| `budget_s` | Seconds the solve may take (600) |

The reply gives `max_von_mises_mpa`, `max_displacement_mm`, `applied_n` and `reaction_n` (they
balance), `safety_factor` and `yield_mpa` where the yield is known, the material used, and the
mesh. A point that lies on no face, or on an edge between two, is refused with its place in the
call (`loads[0].faces[0]`).

## Materials

The built-in metals and plastics carry typical room-temperature handbook values:

| Material | E (MPa) | ν | Yield (MPa) |
| --- | --- | --- | --- |
| Steel | 200 000 | 0.30 | 250 |
| Stainless Steel | 193 000 | 0.29 | 215 |
| Aluminum | 69 000 | 0.33 | 276 |
| Brass | 100 000 | 0.34 | 200 |
| Copper | 117 000 | 0.34 | 70 |
| Titanium | 114 000 | 0.34 | 880 |
| Cast Iron | 110 000 | 0.26 | — (brittle) |
| ABS Plastic | 2 200 | 0.35 | 40 |
| PLA | 3 500 | 0.36 | 50 |
| Nylon | 2 700 | 0.39 | 70 |
| Polycarbonate | 2 400 | 0.37 | 62 |
| Glass | 70 000 | 0.22 | — (brittle) |

Oak and the default material have no elastic data. Give the values yourself, from your own
supplier's data where it matters.

## What it is and isn't

- Linear and small-strain: a stress above the yield means the part yields, not that it carries
  that stress. 3D-printed parts are weaker across their layers than these isotropic values say.
- The stress at a sharp inside corner grows as the mesh gets finer (it is a singularity). Round
  the corner, or read the stress away from it, and use `refine` to see what converges.
- It is checked against beam theory: a 200 mm aluminium cantilever under 10 N bends within 2% of
  P L³ / 3 E I, and its reactions balance the load.
