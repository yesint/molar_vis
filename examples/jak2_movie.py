#!/usr/bin/env python3
"""Render JAK2 and ten inhibitors as one continuous movie.

Run with the freshly built Python module (see docs/PYTHON.md):
    python examples/jak2_movie.py

One MolGroup owns the SD records and shared ligand/contact representations.
Each inhibitor gets a one-second unobstructed camera transition, without a hold.
"""

import argparse
from pathlib import Path
import shutil

import molar_vis as mv


ROOT = Path(__file__).resolve().parents[1]


def render(args):
    if not shutil.which("ffmpeg"):
        raise RuntimeError("ffmpeg is required")
    if not hasattr(mv.Visualizer, "load_group"):
        raise RuntimeError("Rebuild molar_vis with maturin develop for the MolGroup movie API")
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    vis = mv.spawn(visible=args.visible)
    if args.session:
        # Keep the session's display settings, but use the script's camera pose.
        programmed_start = vis.view_json()
        vis.load_session(str(args.session.expanduser().resolve()))
        protein_mol = vis.mols[0]
        inhibitors = vis.groups[0]
        vis.go_to_view(programmed_start, duration=0).wait()
    else:
        protein = mv.System(str(args.protein.resolve()))
        protein_mol = vis.add_mol(protein, source_path=str(args.protein.resolve()))
        cartoon = protein_mol.reps[0]
        cartoon.select(protein("protein"))
        cartoon.style = "cartoon"
        cartoon.color = "ss"
        inhibitors = vis.load_group(str(args.inhibitors.resolve()))
        inhibitors.reps[0].style = "licorice"
        inhibitors.reps[0].color = "element"
        contacts = inhibitors.add_interactions(cartoon)
        contacts.width = 3.0
        vis.projection("orthographic")
        vis.background(1, 1, 1)
        vis.ambient_occlusion(True)
        vis.axes(False)
    if len(inhibitors) < 10:
        raise ValueError("The inhibitor group must contain at least ten molecules")
    movie = vis.movie()
    movie.focus(protein_mol, "protein", duration=0)
    movie.seek(0)
    whole = vis.view_json()

    movie = vis.movie()
    movie.rotate(360, duration=3.0, easing="linear")
    for member in range(10):
        movie.unobstructed_group_member(inhibitors, member, duration=1.0, zoom_out=2.0)
    movie.go_to_view(whole, duration=1.2)
    movie.hold(0.3)  # Show the final whole-protein view.
    print(f"Rendering one continuous movie: {movie.duration:.1f} s", flush=True)
    movie.render(str(output), width=args.width, height=args.height, fps=args.fps, background=False).wait()
    print(f"Saved {output}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--protein", type=Path, default=ROOT / "tests/jak2.pdb")
    parser.add_argument("--inhibitors", type=Path, default=ROOT / "tests/jak2_inhs.sd")
    parser.add_argument("--output", type=Path, default=ROOT / "target/jak2_movie.mp4")
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--fps", type=int, default=30)
    parser.add_argument("--visible", action="store_true")
    parser.add_argument("--session", type=Path, help="Load scene and display settings; keep programmed camera motion")
    args = parser.parse_args()
    if not (1 <= args.width <= 16384 and 1 <= args.height <= 16384 and 1 <= args.fps <= 240):
        parser.error("Size must be 1–16384 pixels; fps must be 1–240")
    render(args)


if __name__ == "__main__":
    main()
