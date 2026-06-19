//! Native binary entry point: parse argv, set up logging, hand off to the core.
//!
//! All native-only concerns (argv, logging, and later file dialogs) live here so
//! that `molar_vis_core` stays compilable to `wasm32-unknown-unknown`.

use std::ffi::OsString;

use molar_vis_core::{parse_file_args, run, AppLaunch};

const USAGE: &str = "\
molar_vis — a modern molecular viewer

USAGE:
    molar_vis [FILES...] [-m FILES...] ...

Files load VMD-style. Within a group the first file provides the topology, and
ALL frames of the group's files form one trajectory (a multi-frame first file
contributes all of its frames). `-m` (or `--molecule`) starts a new molecule.

EXAMPLES:
    molar_vis a.pdb                      one molecule
    molar_vis traj.pdb                   a multi-MODEL file → one molecule, full trajectory
    molar_vis a.pdb a.xtc                a.pdb + the a.xtc trajectory (one molecule)
    molar_vis a.pdb b.pdb                a.pdb with b.pdb loaded as a second frame
    molar_vis -m a.pdb a.xtc -m b.pdb    two molecules, the first with a trajectory

Anything molar can read works (pdb/ent/gro/xyz/tpr + xtc/trr/dcd trajectories).";

fn main() {
    env_logger::init();

    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }

    let files = parse_file_args(args);

    if let Err(err) = run(AppLaunch { files }) {
        eprintln!("molar_vis failed: {err}");
        std::process::exit(1);
    }
}
