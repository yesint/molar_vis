//! Scene selection variables. Molar owns the selection grammar; this module owns
//! the opaque `mN[I].rR[F]` micro-grammar and binds it against the current scene.
use crate::scene::{EvalError, Scene};
use molar::prelude::*;
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};

/// Source numbers follow the top-level molecule tree, with each group one entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceEntry {
    Mol(usize),
    Group(usize),
}
impl Scene {
    pub(crate) fn source_entries(&self) -> Vec<SourceEntry> {
        let mut groups = std::collections::HashSet::new();
        self.molecules
            .iter()
            .enumerate()
            .filter_map(|(mi, mol)| match mol.group {
                None => Some(SourceEntry::Mol(mi)),
                Some(id) if groups.insert(id) => self.group_index(id).map(SourceEntry::Group),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn source_address(&self, mi: usize) -> String {
        for (index, entry) in self.source_entries().iter().enumerate() {
            match *entry {
                SourceEntry::Mol(mol) if mol == mi => return format!("m{index}"),
                SourceEntry::Group(gi) => {
                    let group = &self.groups[gi];
                    if let Some(member) = group
                        .members
                        .iter()
                        .position(|id| self.mol_index(*id) == Some(mi))
                    {
                        return if member == group.current {
                            format!("m{index}")
                        } else {
                            format!("m{index}[{member}]")
                        };
                    }
                }
                _ => (),
            }
        }
        format!("m{mi}")
    }

    pub(crate) fn source_rep_offset(&self, mi: usize) -> usize {
        let mol = &self.molecules[mi];
        if mol.n_shared != 0 {
            return 0;
        }
        mol.group
            .and_then(|id| self.group_index(id))
            .and_then(|gi| {
                let group = &self.groups[gi];
                group
                    .members
                    .get(group.current)
                    .and_then(|id| self.mol_index(*id))
            })
            .map(|shown| self.molecules[shown].n_shared)
            .unwrap_or(0)
    }

    pub(crate) fn visible_depth_bounds(&self) -> Option<[glam::Vec3; 2]> {
        let mut bounds: Option<[glam::Vec3; 2]> = None;
        for mol in self.molecules.iter().filter(|m| m.visible && m.n_atoms > 0) {
            let (mut min, mut max) = mol.current_bbox();
            min -= glam::Vec3::splat(1.0);
            max += glam::Vec3::splat(1.0);
            for rep in mol.reps.iter().filter(|r| r.visible) {
                if let Some(cache) = &rep.mesh_cache {
                    for vertex in &cache.mesh.vertices {
                        let point = glam::Vec3::from_array(vertex.pos);
                        min = min.min(point);
                        max = max.max(point);
                    }
                }
            }
            match &mut bounds {
                Some([lower, upper]) => {
                    *lower = lower.min(min);
                    *upper = upper.max(max);
                }
                None => bounds = Some([min, max]),
            }
        }
        bounds
    }
}

type RepKey = (usize, usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct EvaluationKey {
    mol: usize,
    definition: usize,
    rep: usize,
    frame: Option<usize>,
}
#[derive(Clone, Copy, Debug)]
enum MoleculeTarget {
    Molecule(usize),
    Group { group: usize, member: usize },
}
#[derive(Clone, Copy, Debug)]
struct Target {
    molecule: MoleculeTarget,
    rep: Option<usize>,
    frame: Option<usize>,
}
impl Target {
    fn parse(name: &str) -> Result<Self, SelectionResolveError> {
        fn number(rest: &mut &str) -> Result<usize, SelectionResolveError> {
            let count = rest.bytes().take_while(u8::is_ascii_digit).count();
            if count == 0 {
                return Err(SelectionResolveError::new("expected a nonnegative integer"));
            }
            let result = rest[..count]
                .parse()
                .map_err(|_| SelectionResolveError::new("index is too large"))?;
            *rest = &rest[count..];
            Ok(result)
        }
        fn bracket(rest: &mut &str) -> Result<Option<usize>, SelectionResolveError> {
            if !rest.starts_with('[') {
                return Ok(None);
            }
            *rest = &rest[1..];
            let index = number(rest)?;
            if !rest.starts_with(']') {
                return Err(SelectionResolveError::new("expected ]"));
            }
            *rest = &rest[1..];
            Ok(Some(index))
        }
        let mut rest = name
            .strip_prefix('m')
            .ok_or_else(|| SelectionResolveError::new("expected mN, mG[I], or mN.rR[F]"))?;
        let index = number(&mut rest)?;
        let molecule = match bracket(&mut rest)? {
            Some(member) => MoleculeTarget::Group {
                group: index,
                member,
            },
            None => MoleculeTarget::Molecule(index),
        };
        let (rep, frame) = if let Some(next) = rest.strip_prefix(".r") {
            rest = next;
            (Some(number(&mut rest)?), bracket(&mut rest)?)
        } else {
            (None, None)
        };
        if !rest.is_empty() {
            return Err(SelectionResolveError::new(
                "expected mN, mG[I], mN.rR[F], or mG[I].rR[F]",
            ));
        }
        Ok(Self {
            molecule,
            rep,
            frame,
        })
    }
    fn molecule(self, scene: &Scene) -> Result<usize, SelectionResolveError> {
        let entries = scene.source_entries();
        let index = match self.molecule {
            MoleculeTarget::Molecule(index) => match entries.get(index) {
                Some(SourceEntry::Mol(mi)) => *mi,
                Some(SourceEntry::Group(gi)) => {
                    let group = &scene.groups[*gi];
                    group
                        .members
                        .get(group.current)
                        .and_then(|id| scene.mol_index(*id))
                        .ok_or_else(|| {
                            SelectionResolveError::new("shown group member was deleted")
                        })?
                }
                None => return Err(SelectionResolveError::new(format!("no molecule {index}"))),
            },
            MoleculeTarget::Group { group, member } => {
                let Some(SourceEntry::Group(gi)) = entries.get(group) else {
                    return Err(SelectionResolveError::new(format!(
                        "no molecule group m{group}"
                    )));
                };
                let id = scene.groups[*gi].members.get(member).ok_or_else(|| {
                    SelectionResolveError::new(format!("no member {member} in group m{group}"))
                })?;
                scene
                    .mol_index(*id)
                    .ok_or_else(|| SelectionResolveError::new("group member was deleted"))?
            }
        };
        scene
            .molecules
            .get(index)
            .ok_or_else(|| SelectionResolveError::new(format!("no molecule {index}")))?;
        Ok(index)
    }
    fn key(self, scene: &Scene) -> Result<Option<EvaluationKey>, SelectionResolveError> {
        let mol = self.molecule(scene)?;
        self.rep
            .map(|rep| {
                let (definition, rep) = representation(
                    scene,
                    mol,
                    rep,
                    matches!(self.molecule, MoleculeTarget::Group { .. }),
                )?;
                Ok(EvaluationKey {
                    mol,
                    definition,
                    rep,
                    frame: self.frame,
                })
            })
            .transpose()
    }
}

// Shared group representations are materialized only on the shown member.
// Bind their definitions against any requested member without moving scene data.
fn representation(
    scene: &Scene,
    mi: usize,
    ri: usize,
    group_scope: bool,
) -> Result<RepKey, SelectionResolveError> {
    let mol = &scene.molecules[mi];
    let mut owner = mi;
    let mut index = ri;
    if group_scope && mol.n_shared == 0 {
        if let Some(group) = mol
            .group
            .and_then(|id| scene.group_index(id))
            .map(|gi| &scene.groups[gi])
        {
            if let Some(shown) = group
                .members
                .get(group.current)
                .and_then(|id| scene.mol_index(*id))
            {
                let shared = scene.molecules[shown].n_shared;
                if ri < shared {
                    owner = shown;
                } else {
                    index = ri - shared;
                }
            }
        }
    }
    scene.molecules[owner].reps.get(index).ok_or_else(|| {
        SelectionResolveError::new(format!("no representation {ri} on molecule {mi}"))
    })?;
    Ok((owner, index))
}

/// The table stores parsed descriptions only, never molecular borrows or objects.
#[derive(Debug, Default)]
pub(crate) struct SceneVariables {
    identity: SelectionResolverId,
    targets: Vec<Target>,
}
impl SceneVariables {
    fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}
impl SelectionResolver for SceneVariables {
    fn identity(&self) -> SelectionResolverId {
        self.identity
    }
    fn prepare(&mut self, name: &str) -> Result<SelectionReferenceId, SelectionResolveError> {
        self.targets.push(Target::parse(name)?);
        Ok(self.identity.reference(self.targets.len() - 1))
    }
    fn bind(
        &self,
        _: SelectionReferenceId,
    ) -> Result<SelectionSourceView<'_>, SelectionResolveError> {
        Err(SelectionResolveError::new(
            "a scene binding adapter is required",
        ))
    }
}
struct Program {
    expr: SelectionExpr,
    variables: SceneVariables,
}
fn compile(text: &str) -> Result<Program, EvalError> {
    let mut variables = SceneVariables::default();
    let expr = SelectionExpr::new_with_resolver(text, &mut variables).map_err(parser_error)?;
    Ok(Program { expr, variables })
}
fn parser_error(error: SelectionParserError) -> EvalError {
    match error {
        SelectionParserError::SyntaxError(info) => EvalError::Invalid {
            message: crate::suggest::concise_message(&info),
            span: Some(info.span),
        },
        SelectionParserError::Variable { name, span, error } => EvalError::Invalid {
            message: format!("variable '{name}': {error}"),
            span: Some(span),
        },
        SelectionParserError::ResolverRequired { name, span } => EvalError::Invalid {
            message: format!("variable '{name}' requires a resolver"),
            span: Some(span),
        },
        other => invalid(other.to_string()),
    }
}
fn invalid(message: impl Into<String>) -> EvalError {
    EvalError::Invalid {
        message: message.into(),
        span: None,
    }
}
fn message(error: &EvalError) -> String {
    match error {
        EvalError::Empty => "selection matched no atoms".into(),
        EvalError::Invalid { message, .. } => message.clone(),
    }
}
fn indices(result: Result<Sel, SelectionError>) -> Result<Vec<usize>, EvalError> {
    match result {
        Ok(sel) => Ok(sel.into_svec().to_vec()),
        Err(SelectionError::EmptyExpr(_)) => Ok(Vec::new()),
        Err(SelectionError::Parser(error)) => Err(parser_error(error)),
        Err(error) => Err(invalid(error.to_string())),
    }
}
fn state(scene: &Scene, mol: usize, frame: Option<usize>) -> Result<&State, SelectionResolveError> {
    let mol = scene
        .molecules
        .get(mol)
        .ok_or_else(|| SelectionResolveError::new("molecule was deleted"))?;
    match frame {
        None => Ok(mol.render_state()),
        Some(0) if mol.trajectory.frames.is_empty() => Ok(mol.data.state()),
        Some(index) => mol
            .trajectory
            .frames
            .get(index)
            .ok_or_else(|| SelectionResolveError::new(format!("frame {index} is not loaded"))),
    }
}

struct Binding<'a> {
    scene: &'a Scene,
    variables: &'a SceneVariables,
    results: &'a HashMap<EvaluationKey, Result<Vec<usize>, EvalError>>,
}
impl SelectionResolver for Binding<'_> {
    fn identity(&self) -> SelectionResolverId {
        self.variables.identity
    }
    fn prepare(&mut self, _: &str) -> Result<SelectionReferenceId, SelectionResolveError> {
        Err(SelectionResolveError::new(
            "binding cannot prepare references",
        ))
    }
    fn bind(
        &self,
        id: SelectionReferenceId,
    ) -> Result<SelectionSourceView<'_>, SelectionResolveError> {
        let target = self
            .variables
            .targets
            .get(id.slot())
            .ok_or_else(|| SelectionResolveError::new("unknown prepared reference"))?;
        let mi = target.molecule(self.scene)?;
        let mol = &self.scene.molecules[mi];
        let domain = if let Some(rep) = target.rep {
            let (definition, rep) = representation(
                self.scene,
                mi,
                rep,
                matches!(target.molecule, MoleculeTarget::Group { .. }),
            )?;
            let key = EvaluationKey {
                mol: mi,
                definition,
                rep,
                frame: target.frame,
            };
            match self.results.get(&key) {
                Some(Ok(indices)) => Some(indices.as_slice()),
                Some(Err(error)) => return Err(SelectionResolveError::new(message(error))),
                None => {
                    return Err(SelectionResolveError::new(
                        "representation dependency was not evaluated",
                    ))
                }
            }
        } else {
            None
        };
        SelectionSourceView::from_parts(
            mol.data.topology(),
            state(self.scene, mi, target.frame)?,
            domain,
        )
        .map_err(|error| SelectionResolveError::new(error.to_string()))
    }
}

type Programs = HashMap<RepKey, Result<Program, EvalError>>;
struct Evaluation<'a> {
    scene: &'a Scene,
    programs: &'a Programs,
    results: HashMap<EvaluationKey, Result<Vec<usize>, EvalError>>,
    stack: Vec<EvaluationKey>,
}
impl<'a> Evaluation<'a> {
    fn new(scene: &'a Scene, programs: &'a Programs) -> Self {
        Self {
            scene,
            programs,
            results: HashMap::new(),
            stack: Vec::new(),
        }
    }
    fn dependencies(&mut self, variables: &SceneVariables) {
        for target in &variables.targets {
            if let Ok(Some(key)) = target.key(self.scene) {
                let _ = self.rep(key);
            }
        }
    }
    fn query(
        &mut self,
        mi: usize,
        frame: Option<usize>,
        expr: &SelectionExpr,
        variables: &SceneVariables,
    ) -> Result<Vec<usize>, EvalError> {
        self.dependencies(variables);
        let source = SelectionSourceView::from_parts(
            self.scene.molecules[mi].data.topology(),
            state(self.scene, mi, frame).map_err(|e| invalid(e.to_string()))?,
            None,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let binding = Binding {
            scene: self.scene,
            variables,
            results: &self.results,
        };
        indices(expr.evaluate(&source, &binding))
    }
    fn rep(&mut self, key: EvaluationKey) -> Result<(), EvalError> {
        if let Some(result) = self.results.get(&key) {
            return result.as_ref().map(|_| ()).map_err(Clone::clone);
        }
        if self.stack.contains(&key) {
            let mut path = self
                .stack
                .iter()
                .map(|k| {
                    format!(
                        "m{}.r{}{}",
                        k.mol,
                        k.rep,
                        k.frame.map(|f| format!("[{f}]")).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>();
            path.push(format!("m{}.r{}", key.mol, key.rep));
            let error = invalid(format!(
                "cyclic representation reference: {}",
                path.join(" -> ")
            ));
            self.results.insert(key, Err(error.clone()));
            return Err(error);
        }
        self.stack.push(key);
        let scene = self.scene;
        let programs = self.programs;
        let result = (|| {
            let rep = scene
                .molecules
                .get(key.definition)
                .and_then(|m| m.reps.get(key.rep))
                .ok_or_else(|| {
                    invalid(format!(
                        "no representation {} on molecule {}",
                        key.rep, key.mol
                    ))
                })?;
            let (expr, variables) = match programs.get(&(key.definition, key.rep)) {
                Some(Ok(program)) => (&program.expr, &program.variables),
                Some(Err(error)) => return Err(error.clone()),
                None => (
                    rep.expr
                        .as_ref()
                        .ok_or_else(|| invalid("representation has no compiled expression"))?,
                    rep.selection_variables
                        .as_ref()
                        .ok_or_else(|| invalid("representation has no prepared references"))?,
                ),
            };
            // A static rep's current membership is the membership it displays.
            // Explicit frames always evaluate the expression at the requested frame.
            if key.frame.is_none()
                && !rep.sel_dirty
                && !rep.dynamic
                && variables.is_empty()
                && key.definition == key.mol
                && !programs.contains_key(&(key.definition, key.rep))
            {
                if let Some(sel) = &rep.sel {
                    return Ok(sel.get_index_slice().to_vec());
                }
                if rep.sel_empty {
                    return Ok(Vec::new());
                }
            }
            self.query(key.mol, key.frame, expr, variables)
        })();
        self.stack.pop();
        let status = result.as_ref().map(|_| ()).map_err(Clone::clone);
        self.results.insert(key, result);
        status
    }
}
fn programs(scene: &Scene) -> Programs {
    scene
        .molecules
        .iter()
        .enumerate()
        .flat_map(|(mi, m)| {
            m.reps.iter().enumerate().filter_map(move |(ri, r)| {
                let needs_compile = r.expr.is_none()
                    || r.selection_variables.is_none()
                    || (r.sel_dirty
                        && r.expr
                            .as_ref()
                            .is_some_and(|expr| expr.get_str() != r.sel_text.trim()));
                needs_compile.then(|| ((mi, ri), compile(&r.sel_text)))
            })
        })
        .collect()
}
fn signature(scene: &Scene) -> u64 {
    let mut hasher = DefaultHasher::new();
    for mol in &scene.molecules {
        mol.id.hash(&mut hasher);
        mol.n_atoms.hash(&mut hasher);
        mol.trajectory.current.hash(&mut hasher);
        mol.trajectory.frames.len().hash(&mut hasher);
        mol.data.coords_version().hash(&mut hasher);
        for rep in &mol.reps {
            rep.id.hash(&mut hasher);
            rep.dynamic.hash(&mut hasher);
            rep.sel_text.hash(&mut hasher);
        }
    }
    for group in &scene.groups {
        group.id.hash(&mut hasher);
        group.members.hash(&mut hasher);
        group.current.hash(&mut hasher);
    }
    hasher.finish()
}

impl Scene {
    /// Evaluate a query against a molecule's displayed frame and the current scene.
    pub fn select(&self, molecule: usize, text: &str) -> Result<Sel, EvalError> {
        if self.molecules.get(molecule).is_none() {
            return Err(invalid(format!("no molecule {molecule}")));
        }
        let program = compile(text)?;
        let programs = programs(self);
        let mut evaluation = Evaluation::new(self, &programs);
        let ids = evaluation.query(molecule, None, &program.expr, &program.variables)?;
        Sel::from_vec(ids).map_err(|_| EvalError::Empty)
    }
    /// Validate a pending representation edit, including cycles through that edit.
    pub(crate) fn validate_selection(
        &self,
        molecule: usize,
        rep: usize,
        text: &str,
    ) -> Result<(), EvalError> {
        let mut programs = programs(self);
        programs.insert((molecule, rep), compile(text));
        let mut evaluation = Evaluation::new(self, &programs);
        evaluation.rep(EvaluationKey {
            mol: molecule,
            definition: molecule,
            rep,
            frame: None,
        })?;
        Ok(())
    }
    /// Evaluate before borrowing molecules mutably for geometry. No GPU is needed.
    pub(crate) fn refresh_selections(&mut self) -> bool {
        self.refresh_selections_impl(None)
    }

    pub(crate) fn refresh_selections_for(
        &mut self,
        requested: &std::collections::HashSet<RepKey>,
    ) -> bool {
        self.refresh_selections_impl(Some(requested))
    }

    fn refresh_selections_impl(
        &mut self,
        requested: Option<&std::collections::HashSet<RepKey>>,
    ) -> bool {
        let next_signature = signature(self);
        let scene_changed = next_signature != self.selection_signature
            || self.molecules.iter().any(|m| {
                m.reps
                    .iter()
                    .any(|r| r.sel_dirty || r.coords_dirty || r.geom_dirty)
            });
        if scene_changed {
            self.selection_revision = self.selection_revision.wrapping_add(1);
        }
        let revision = self.selection_revision;
        let roots = self
            .molecules
            .iter()
            .enumerate()
            .flat_map(|(mi, m)| {
                m.reps.iter().enumerate().filter_map(move |(ri, r)| {
                    if requested.is_some_and(|roots| !roots.contains(&(mi, ri))) {
                        return None;
                    }
                    let has_variables = r
                        .selection_variables
                        .as_ref()
                        .is_some_and(|v| !v.is_empty())
                        && r.expr
                            .as_ref()
                            .is_some_and(|expr| expr.get_str() == r.sel_text.trim());
                    (r.sel_dirty || (has_variables && r.selection_scene_revision != revision))
                        .then_some(EvaluationKey {
                            mol: mi,
                            definition: mi,
                            rep: ri,
                            frame: None,
                        })
                })
            })
            .collect::<Vec<_>>();
        if roots.is_empty() {
            self.selection_signature = next_signature;
            return false;
        }
        let _timing = crate::performance::span("selection");
        let programs = programs(self);
        let results = {
            let mut evaluation = Evaluation::new(self, &programs);
            for key in &roots {
                let _ = evaluation.rep(*key);
            }
            roots
                .iter()
                .map(|key| (*key, evaluation.results.remove(key).unwrap()))
                .collect::<Vec<_>>()
        };
        // Retain prepared definitions even for dependency-only hidden reps.
        // Their selection and dirty flags are updated only when they are roots.
        for ((mi, ri), program) in programs {
            if let Ok(program) = program {
                let rep = &mut self.molecules[mi].reps[ri];
                rep.expr = Some(program.expr);
                rep.selection_variables = Some(program.variables);
            }
        }
        let mut changed = false;
        for (key, result) in results {
            let mol = &mut self.molecules[key.mol];
            let rep = &mut mol.reps[key.rep];
            let dirty = rep.sel_dirty;
            match result {
                Ok(ids) if !ids.is_empty() => {
                    let same = rep.sel.as_ref().is_some_and(|s| s.get_index_slice() == ids);
                    if !same || dirty {
                        rep.geom_dirty = true;
                        changed = true;
                    }
                    if !same {
                        rep.sel = Some(Sel::from_vec(ids).unwrap());
                        mol.glow_dirty = true;
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            mol.pick_dirty = true;
                        }
                    }
                    rep.sel_error = None;
                    rep.sel_error_span = None;
                    rep.sel_empty = false;
                }
                Ok(_) => {
                    changed |= rep.sel.is_some();
                    mol.glow_dirty = true;
                    rep.sel = None;
                    rep.sel_error = None;
                    rep.sel_error_span = None;
                    rep.sel_empty = true;
                    rep.gpu = Default::default();
                    rep.primitive_cache = None;
                    rep.cartoon_cache = None;
                    rep.mesh_cache = None;
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        rep.geometry_job = None;
                        rep.geometry_waiting = false;
                        mol.pick_dirty = true;
                    }
                    rep.cache_geometry(Default::default(), 0, false, true);
                }
                Err(EvalError::Invalid { message, span }) => {
                    let lead =
                        rep.sel_text.len() - rep.sel_text.trim_start_matches([' ', '\t']).len();
                    rep.sel_error = Some(message);
                    rep.sel_error_span = span.map(|s| s.start + lead..s.end + lead);
                    rep.sel_empty = false;
                }
                Err(EvalError::Empty) => unreachable!("intermediate empty sets are valid"),
            }
            rep.sel_dirty = false;
            rep.selection_scene_revision = revision;
        }
        self.selection_signature = next_signature;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        data::RawMolecule,
        geometry::RepKind,
        scene::{MoleculeSource, Representation},
        settings::RepDefaults,
    };
    fn raw(xs: &[f32], names: &[&str]) -> RawMolecule {
        let mut top = Topology::default();
        for name in names {
            top.atoms.push(&Atom::new().with_name(name).with_mass(1.0));
        }
        top.assign_resindex();
        let state = State {
            coords: xs.iter().map(|x| Pos::new(*x, 0.0, 0.0)).collect(),
            ..Default::default()
        };
        RawMolecule {
            name: "test".into(),
            source: MoleculeSource::default(),
            system: System::new(top, state).unwrap(),
            n_atoms: xs.len(),
            bonds: vec![],
            bbox_min: glam::Vec3::ZERO,
            bbox_max: glam::Vec3::splat(5.0),
        }
    }
    fn rep(text: &str) -> Representation {
        let mut rep = Representation::new(RepKind::Vdw);
        rep.sel_text = text.into();
        rep
    }
    fn scene() -> Scene {
        let mut scene = Scene::default();
        let defaults = RepDefaults::default();
        scene.add(raw(&[0.0, 2.0, 5.0], &["CA", "CB", "CA"]), &defaults);
        scene.add(raw(&[5.0, 0.1], &["CB", "CA"]), &defaults);
        scene.molecules[0].reps = vec![rep("within 0.5 of m1:(name CA)")];
        scene.molecules[1].reps = vec![rep("name CA")];
        scene
    }
    fn selected(scene: &Scene, mi: usize, ri: usize) -> Vec<usize> {
        scene.molecules[mi].reps[ri]
            .sel
            .as_ref()
            .map(|s| s.get_index_slice().to_vec())
            .unwrap_or_default()
    }
    fn ids(sel: Sel) -> Vec<usize> {
        sel.into_svec().to_vec()
    }
    fn settle(scene: &mut Scene) {
        for mol in &mut scene.molecules {
            for rep in &mut mol.reps {
                rep.geom_dirty = false;
                rep.coords_dirty = false;
            }
        }
    }
    #[test]
    fn cross_molecule_predicates_and_queries_use_foreign_coordinates() {
        let mut scene = scene();
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [0]);
        assert_eq!(ids(scene.select(0, "within 0.5 of m1.r0:").unwrap()), [0]);
        assert_eq!(
            ids(scene
                .select(0, "within 0.5 of (m1:(name CA) or m1:(name CB))")
                .unwrap()),
            [0, 2]
        );
    }
    #[test]
    fn references_rebind_after_coordinates_change_without_repreparing() {
        let mut scene = scene();
        scene.refresh_selections();
        settle(&mut scene);
        let identity = scene.molecules[0].reps[0]
            .selection_variables
            .as_ref()
            .unwrap()
            .identity;
        let mut frame = scene.molecules[1].data.state().clone();
        frame.coords[1].x = 2.1;
        scene.molecules[1].data.set_state(frame).unwrap();
        scene.molecules[1].reps[0].coords_dirty = true;
        assert!(scene.refresh_selections());
        assert_eq!(selected(&scene, 0, 0), [1]);
        assert_eq!(
            identity,
            scene.molecules[0].reps[0]
                .selection_variables
                .as_ref()
                .unwrap()
                .identity
        );
        settle(&mut scene);
        assert!(!scene.refresh_selections());
    }
    #[test]
    fn reference_selection_edits_are_order_independent() {
        let mut scene = scene();
        scene.molecules[0].reps[0].sel_text = "within 0.5 of m1.r0:".into();
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [0]);
        settle(&mut scene);
        scene.molecules[1].reps[0].sel_text = "name CB".into();
        scene.molecules[1].reps[0].sel_dirty = true;
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [2]);
    }
    #[test]
    fn displayed_and_explicit_frames_use_matching_membership() {
        let mut scene = scene();
        scene.molecules[1]
            .data
            .set_state(State {
                coords: vec![Pos::new(0.1, 0.0, 0.0), Pos::new(2.1, 0.0, 0.0)],
                ..Default::default()
            })
            .unwrap();
        scene.molecules[1].reps[0].sel_text = "x < 1".into();
        scene.molecules[1].trajectory.frames = vec![
            scene.molecules[1].data.state().clone(),
            State {
                coords: vec![Pos::new(2.1, 0.0, 0.0), Pos::new(0.1, 0.0, 0.0)],
                ..Default::default()
            },
        ];
        scene.molecules[0].reps[0].sel_text = "within 0.5 of m1.r0:".into();
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [0]);
        settle(&mut scene);
        scene.molecules[1].trajectory.current = 1;
        scene.molecules[1].apply_current_frame();
        scene.refresh_selections();
        assert_eq!(
            selected(&scene, 0, 0),
            [1],
            "static current membership follows its atoms"
        );
        assert_eq!(
            ids(scene.select(0, "within 0.5 of m1.r0[1]:").unwrap()),
            [0],
            "explicit frame evaluates selection at that frame"
        );
        scene.molecules[1].reps[0].dynamic = true;
        scene.molecules[1].reps[0].sel_dirty = true;
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [0]);
    }
    #[test]
    fn default_selection_uses_displayed_frame_even_without_system_swap() {
        let mut scene = scene();
        scene.molecules[0].trajectory.frames = vec![State {
            coords: vec![
                Pos::new(0.0, 0.0, 0.0),
                Pos::new(0.0, 0.0, 0.0),
                Pos::new(0.0, 0.0, 0.0),
            ],
            ..Default::default()
        }];
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [0, 1, 2]);
    }
    #[test]
    fn deletion_errors_preserve_geometry_and_recovery_rebinds() {
        let mut scene = scene();
        scene.molecules[0].reps[0].sel_text = "  within 0.5 of m1:(name CA)".into();
        scene.refresh_selections();
        settle(&mut scene);
        scene.molecules.pop();
        scene.refresh_selections();
        let rep = &scene.molecules[0].reps[0];
        assert_eq!(selected(&scene, 0, 0), [0]);
        assert!(rep.sel_error.as_ref().unwrap().contains("no molecule 1"));
        assert_eq!(&rep.sel_text[rep.sel_error_span.clone().unwrap()], "m1");
        scene.add(raw(&[2.1], &["CA"]), &RepDefaults::default());
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [1]);
        assert!(scene.molecules[0].reps[0].sel_error.is_none());
    }
    #[test]
    fn empty_reference_can_become_nonempty_again() {
        let mut scene = scene();
        scene.molecules[0].reps[0].sel_text = "within 0.5 of m1.r0:".into();
        scene.molecules[1].reps[0].sel_text = "name NEVER".into();
        scene.refresh_selections();
        assert!(scene.molecules[0].reps[0].sel_empty);
        assert!(scene.molecules[1].reps[0].sel_empty);
        let identity = scene.molecules[0].reps[0]
            .selection_variables
            .as_ref()
            .unwrap()
            .identity;
        scene.molecules[1].reps[0].sel_text = "name CA".into();
        scene.molecules[1].reps[0].sel_dirty = true;
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [0]);
        assert_eq!(
            identity,
            scene.molecules[0].reps[0]
                .selection_variables
                .as_ref()
                .unwrap()
                .identity
        );
    }
    #[test]
    fn cycles_and_invalid_edits_produce_errors_without_recursion() {
        let mut scene = scene();
        scene.refresh_selections();
        assert!(matches!(
            scene.validate_selection(0, 0, "within 0.5 of m0.r0:"),
            Err(EvalError::Invalid { .. })
        ));
        scene.molecules[0].reps[0].sel_text = "within 0.5 of m1.r0:".into();
        scene.molecules[0].reps[0].sel_dirty = true;
        scene.molecules[1].reps[0].sel_text = "within 0.5 of m0.r0:".into();
        scene.molecules[1].reps[0].sel_dirty = true;
        scene.refresh_selections();
        assert!(scene.molecules[0].reps[0]
            .sel_error
            .as_ref()
            .unwrap()
            .contains("cyclic"));
        assert!(scene.molecules[1].reps[0]
            .sel_error
            .as_ref()
            .unwrap()
            .contains("cyclic"));
    }
    #[test]
    fn group_member_shared_representations_work_on_unshown_members() {
        let mut scene = scene();
        scene.molecules.pop();
        scene.add_group(
            vec![
                raw(&[0.1, 5.0], &["CA", "CB"]),
                raw(&[5.0, 2.1], &["CB", "CA"]),
            ],
            MoleculeSource::default(),
            "group".into(),
            &RepDefaults::default(),
        );
        scene.molecules[1].reps[0].sel_text = "name CA".into();
        scene.molecules[0].reps[0].sel_text = "within 0.5 of m1[1].r0:".into();
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [1]);
        assert_eq!(
            ids(scene.select(0, "within 0.5 of m1[0].r0:").unwrap()),
            [0]
        );
        scene.switch_group_member(0, 1);
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [1]);
    }
    #[test]
    fn missing_reps_frames_and_malformed_names_have_spans() {
        let scene = scene();
        for text in [
            "within 0.5 of m1.r99:",
            "within 0.5 of m1.r0[99]:",
            "within 0.5 of m9[0]:",
            "within 0.5 of m1.whatever:",
            "within 0.5 of m99999999999999999999999:",
        ] {
            assert!(
                matches!(
                    scene.select(0, text),
                    Err(EvalError::Invalid { span: Some(_), .. })
                ),
                "{text}"
            );
        }
        assert!(Target::parse("m0.r0[0]").is_ok());
        for name in ["m-1", "m0.r-1", "m0[", "m0[]", "m0[1", "m0.r0[3]x"] {
            assert!(Target::parse(name).is_err(), "{name}");
        }
    }
    #[test]
    fn script_select_and_focus_accept_scene_variables() {
        let mut scene = scene();
        let mut camera = crate::camera::Camera::default();
        let defaults = RepDefaults::default();
        let result = crate::script::apply_scene_command(
            &mut scene,
            &mut camera,
            &defaults,
            crate::script::Command::Select {
                mol: 0,
                rep: crate::script::RepRef::Index(0),
                text: "within 0.5 of m1.r0:".into(),
            },
        );
        assert!(result.is_ok());
        scene.refresh_selections();
        assert_eq!(selected(&scene, 0, 0), [0]);
        assert!(crate::script::apply_scene_command(
            &mut scene,
            &mut camera,
            &defaults,
            crate::script::Command::Focus {
                mol: 0,
                text: "within 0.5 of m1:".into()
            }
        )
        .is_ok());
    }
}

#[cfg(test)]
mod restricted_refresh_tests {
    use super::*;
    use crate::{
        data::RawMolecule, geometry::RepKind, scene::Representation, settings::RepDefaults,
    };
    #[test]
    fn deferred_roots_keep_structural_invalidation_after_subset_refresh() {
        let mut scene = Scene::default();
        let defaults = RepDefaults::default();
        for name in ["target", "source"] {
            scene.add(
                RawMolecule::single_atom(name, Atom::new().with_name("CA"), glam::Vec3::ZERO)
                    .unwrap(),
                &defaults,
            );
        }
        let mut rep = Representation::new(RepKind::Vdw);
        rep.sel_text = "within 0.5 of m1:".into();
        scene.molecules[0].reps.push(rep);
        scene.refresh_selections();
        for mol in &mut scene.molecules {
            for rep in &mut mol.reps {
                rep.geom_dirty = false;
                rep.coords_dirty = false;
            }
        }
        scene.molecules.pop();
        scene.refresh_selections_for(&[(0, 0)].into_iter().collect());
        assert!(scene.molecules[0].reps[1].sel_error.is_none());
        scene.refresh_selections();
        assert!(scene.molecules[0].reps[1]
            .sel_error
            .as_ref()
            .unwrap()
            .contains("no molecule 1"));
    }
}

#[cfg(test)]
mod preparation_cache_tests {
    use super::*;
    use crate::{
        data::RawMolecule, geometry::RepKind, scene::Representation, settings::RepDefaults,
    };
    fn scene() -> Scene {
        let mut scene = Scene::default();
        let defaults = RepDefaults::default();
        for name in ["target", "source"] {
            scene.add(
                RawMolecule::single_atom(name, Atom::new().with_name("CA"), glam::Vec3::ZERO)
                    .unwrap(),
                &defaults,
            );
        }
        let mut rep = Representation::new(RepKind::Vdw);
        rep.sel_text = "within 0.5 of m1.r0:".into();
        scene.molecules[0].reps = vec![rep];
        scene
    }
    #[test]
    fn dependency_only_reps_keep_prepared_definitions_and_dirty_flags() {
        let mut scene = scene();
        let roots = [(0, 0)].into_iter().collect();
        scene.refresh_selections_for(&roots);
        let identity = scene.molecules[1].reps[0]
            .selection_variables
            .as_ref()
            .unwrap()
            .identity;
        assert!(scene.molecules[1].reps[0].sel_dirty);
        scene.refresh_selections_for(&roots);
        assert_eq!(
            identity,
            scene.molecules[1].reps[0]
                .selection_variables
                .as_ref()
                .unwrap()
                .identity
        );
    }
    #[test]
    fn invalid_draft_error_survives_unrelated_scene_refresh() {
        let mut scene = scene();
        scene.refresh_selections();
        scene.molecules[0].reps[0].sel_text = "bad_expression".into();
        scene.molecules[0].reps[0].sel_dirty = true;
        scene.refresh_selections();
        let error = scene.molecules[0].reps[0].sel_error.clone();
        assert!(error.is_some());
        scene.molecules[1].reps[0].coords_dirty = true;
        scene.refresh_selections();
        assert_eq!(scene.molecules[0].reps[0].sel_error, error);
    }
}

#[cfg(test)]
mod group_index_tests {
    use super::*;
    use crate::{
        data::RawMolecule,
        geometry::RepKind,
        scene::{MoleculeSource, Representation},
        settings::RepDefaults,
    };
    #[test]
    fn group_references_include_shared_prefix_before_member_reps() {
        let mut scene = Scene::default();
        let defaults = RepDefaults::default();
        scene.add(
            RawMolecule::single_atom("default", Atom::new().with_name("CA"), glam::Vec3::ZERO)
                .unwrap(),
            &defaults,
        );
        let records = ["member0", "member1"]
            .into_iter()
            .map(|name| {
                RawMolecule::single_atom(name, Atom::new().with_name("CA"), glam::Vec3::ZERO)
                    .unwrap()
            })
            .collect();
        scene.add_group(
            records,
            MoleculeSource::default(),
            "group".into(),
            &defaults,
        );
        let mut own = Representation::new(RepKind::Vdw);
        own.sel_text = "name NEVER".into();
        scene.molecules[2].reps.push(own);
        assert!(scene.select(0, "within 0.5 of m1.r0").is_ok());
        assert!(scene.select(0, "within 0.5 of m1[1].r0:").is_ok());
        assert!(matches!(
            scene.select(0, "within 0.5 of m1[1].r1:"),
            Err(EvalError::Empty)
        ));
    }
}

#[cfg(test)]
mod tree_source_tests {
    use super::*;
    use crate::{data::RawMolecule, scene::MoleculeSource, settings::RepDefaults};
    #[test]
    fn group_source_follows_shown_pose_and_later_sources_keep_tree_numbers() {
        let mut scene = Scene::default();
        let defaults = RepDefaults::default();
        let atom = || Atom::new().with_name("CA");
        scene.add(
            RawMolecule::single_atom("protein", atom(), glam::Vec3::ZERO).unwrap(),
            &defaults,
        );
        scene.add_group(
            vec![
                RawMolecule::single_atom("pose0", atom(), glam::Vec3::ZERO).unwrap(),
                RawMolecule::single_atom("pose1", atom(), glam::Vec3::X).unwrap(),
            ],
            MoleculeSource::default(),
            "poses".into(),
            &defaults,
        );
        scene.add(
            RawMolecule::single_atom("last", atom(), glam::Vec3::ZERO).unwrap(),
            &defaults,
        );
        assert_eq!(
            scene.source_entries(),
            [
                SourceEntry::Mol(0),
                SourceEntry::Group(0),
                SourceEntry::Mol(3)
            ]
        );
        assert_eq!(Target::parse("m1.r0").unwrap().molecule(&scene).unwrap(), 1);
        assert!(scene.select(0, "within 0.1 of m1.r0").is_ok());
        scene.switch_group_member(0, 1);
        assert_eq!(Target::parse("m1.r0").unwrap().molecule(&scene).unwrap(), 2);
        assert!(matches!(
            scene.select(0, "within 0.1 of m1.r0"),
            Err(EvalError::Empty)
        ));
        assert!(scene.select(0, "within 0.1 of m1[0].r0").is_ok());
        assert_eq!(Target::parse("m2").unwrap().molecule(&scene).unwrap(), 3);
        assert_eq!(scene.source_address(2), "m1");
        assert_eq!(scene.source_address(1), "m1[0]");
    }
}
