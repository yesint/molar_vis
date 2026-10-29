//! Group navigation and stable handles for external hosts and movie timelines.
use super::*;
use crate::scene::GroupId;

impl App {
    /// Show a member from the UI, applying the optional camera centering setting.
    pub(super) fn show_group_member_from_ui(&mut self, gi: usize, member: usize) {
        self.scene.switch_group_member(gi, member);
        if let Some(&id) = self.scene.groups[gi].members.get(member)
            .filter(|_| self.settings.behavior.group_switch_zoom)
        {
            if let Some(mi) = self.scene.mol_index(id) {
                let (min, max) = self.scene.molecules[mi].current_bbox();
                self.center_camera_on(0.5 * (min + max));
            }
        }
        self.view_dirty = true;
    }

    pub(super) fn handle_group_navigation(&mut self, ctx: &egui::Context) {
        if self.scene.groups.len() != 1 || ctx.egui_wants_keyboard_input() {
            return;
        }
        let group = &self.scene.groups[0];
        let count = group.members.len();
        if count < 2 {
            return;
        }
        let next = ctx.input_mut(|i| {
            if !i.modifiers.is_none() {
                return None;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                Some((group.current + count - 1) % count)
            } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                Some((group.current + 1) % count)
            } else {
                None
            }
        });
        if let Some(member) = next {
            self.show_group_member_from_ui(0, member);
        }
    }

    /// Add all parsed file records as a normal viewer group with shared representations.
    pub fn add_shared_group(
        &mut self,
        sources: Vec<(String, Box<dyn crate::moldata::SharedSource>)>,
        name: String,
    ) -> Result<u64, String> {
        let was_empty = self.scene.molecules.is_empty();
        let first = self.scene.add_shared_group(sources, name, &self.settings.behavior.bond_params(), &self.rep_defaults)?;
        let mi = self.scene.mol_index(first).ok_or("group has no first member")?;
        self.scene.selected_mol = Some(mi);
        if was_empty {
            let (min, max) = self.scene.molecules[mi].current_bbox();
            self.reframe_camera(min, max);
        }
        let group = self.scene.molecules[mi].group.ok_or("molecule has no group")?;
        let gi = self.scene.group_index(group).ok_or("group was not added")?;
        self.scene.groups[gi].expanded = true;
        self.scene.groups[gi].members_expanded = true;
        self.view_dirty = true;
        Ok(group.0)
    }

    pub fn group_ids(&self) -> Vec<u64> {
        self.scene.groups.iter().map(|g| g.id.0).collect()
    }

    pub fn resolve_group(&self, id: u64) -> Result<usize, String> {
        self.scene
            .group_index(GroupId(id))
            .ok_or_else(|| format!("stale group handle {id}"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_group(&mut self, path: &str) -> Result<u64, String> {
        let path = PathBuf::from(path);
        let records = data::load_records(&path, &self.settings.behavior.bond_params())?;
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.add_group(records, MoleculeSource::File(path), name);
        Ok(self.scene.groups.last().expect("nonempty records").id.0)
    }

    pub fn group_members(&self, group: u64) -> Result<Vec<u64>, String> {
        let gi = self.resolve_group(group)?;
        Ok(self.scene.groups[gi].members.iter().map(|m| m.0).collect())
    }

    pub fn group_current(&self, group: u64) -> Result<usize, String> {
        Ok(self.scene.groups[self.resolve_group(group)?].current)
    }

    pub fn show_group_member(&mut self, group: u64, member: usize) -> Result<(), String> {
        let gi = self.resolve_group(group)?;
        if member >= self.scene.groups[gi].members.len() {
            return Err("group member is out of range".into());
        }
        if self
            .scene
            .mol_index(self.scene.groups[gi].members[member])
            .is_none()
        {
            return Err("group member was removed".into());
        }
        self.view_dirty |= self.scene.switch_group_member(gi, member);
        Ok(())
    }

    pub fn group_rep_ids(&self, group: u64) -> Result<(u64, Vec<u64>), String> {
        let gi = self.resolve_group(group)?;
        let g = &self.scene.groups[gi];
        let mi = self
            .scene
            .mol_index(g.members[g.current])
            .ok_or("group member was removed")?;
        let m = &self.scene.molecules[mi];
        Ok((
            m.id.0,
            m.reps[..m.n_shared.min(m.reps.len())]
                .iter()
                .map(|r| r.id.0)
                .collect(),
        ))
    }

    pub fn add_group_interactions_with_id(
        &mut self,
        group: u64,
        rep: u64,
        partner_mol: u64,
        partner_rep: u64,
    ) -> Result<(), String> {
        let gi = self.resolve_group(group)?;
        let g = &self.scene.groups[gi];
        let mi = self
            .scene
            .mol_index(g.members[g.current])
            .ok_or("group member was removed")?;
        let (pm, pr) = self.resolve_rep(partner_mol, partner_rep)?;
        let ri = self.add_interactions_rep(mi, pm, pr)?;
        let mol = &mut self.scene.molecules[mi];
        let mut interaction = mol.reps.remove(ri);
        interaction.id = scene::RepId(rep);
        mol.reps.insert(mol.n_shared, interaction);
        mol.n_shared += 1;
        Ok(())
    }
}
