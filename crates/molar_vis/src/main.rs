//! Native binary entry point: parse argv, set up logging, hand off to the core.
//!
//! All native-only concerns (argv, logging, and later file dialogs) live here so
//! that `molar_vis_core` stays compilable to `wasm32-unknown-unknown`.

use std::ffi::OsString;

use molar_vis_core::{parse_launch_args, run};

const USAGE: &str = "\
molar_vis — a modern molecular viewer

USAGE:
    molar_vis [FILES...] [-m FILES...] ...
    molar_vis -r RECEPTOR_FILES... -l LIGAND_FILES...

Files load VMD-style. Within a group the first file provides the topology, and
ALL frames of the group's files form one trajectory (a multi-frame first file
contributes all of its frames). `-m` (or `--molecule`) starts a new molecule.

Docking mode: -r (or --receptor) takes a receptor structure and optional trajectory
files. -l (or --ligand) takes one multi-record pose file or several ligand files.
Both flags are required; order is arbitrary. Docking flags cannot be mixed with
ordinary molecule arguments. Use -- before filenames beginning with '-'.

EXAMPLES:
    molar_vis -r receptor.pdb -l poses.sdf    docking poses
    molar_vis -r receptor.pdb receptor.xtc -l pose1.pdb pose2.pdb
    molar_vis a.pdb                      one molecule
    molar_vis traj.pdb                   a multi-MODEL file → one molecule, full trajectory
    molar_vis a.pdb a.xtc                a.pdb + the a.xtc trajectory (one molecule)
    molar_vis a.pdb b.pdb                a.pdb with b.pdb loaded as a second frame
    molar_vis -m a.pdb a.xtc -m b.pdb    two molecules, the first with a trajectory

Anything molar can read works (pdb/ent/gro/xyz/tpr + xtc/trr/dcd trajectories).";

fn main() {
    env_logger::init();

    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args
        .iter()
        .take_while(|a| a.as_os_str() != "--")
        .any(|a| a == "-h" || a == "--help")
    {
        println!("{USAGE}");
        return;
    }

    let launch = match parse_launch_args(args) {
        Ok(launch) => launch,
        Err(err) => {
            eprintln!("molar_vis: {err}\nUse --help for usage.");
            std::process::exit(2);
        }
    };

    if let Err(err) = run(launch) {
        eprintln!("molar_vis failed: {err}");
        std::process::exit(1);
    }
}
