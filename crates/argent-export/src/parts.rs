//! Per-triangle part ids, tolerant of meshes whose `parts` are empty or
//! leave triangles uncovered.

use argent_mesh::Mesh;

pub(crate) struct PartTable {
    pub names: Vec<String>,
    pub materials: Vec<Option<String>>,
    /// Part index of every triangle of the mesh.
    pub tri_part: Vec<u32>,
    /// Triangles per part.
    pub counts: Vec<usize>,
}

impl PartTable {
    /// One entry per `mesh.parts` element, in order (empty parts included,
    /// so indices line up with the mesh). Triangles no part claims go to an
    /// extra part, named `fallback` when the mesh has no parts at all.
    pub(crate) fn new(mesh: &Mesh, fallback: &str) -> PartTable {
        let nt = mesh.triangles.len();
        let mut tri_part = vec![u32::MAX; nt];
        let mut names = Vec::with_capacity(mesh.parts.len() + 1);
        let mut materials = Vec::with_capacity(mesh.parts.len() + 1);
        for (i, p) in mesh.parts.iter().enumerate() {
            names.push(p.name.clone());
            materials.push(p.material.clone());
            let s = (p.triangles.start as usize).min(nt);
            let e = (p.triangles.end as usize).min(nt);
            for slot in &mut tri_part[s..e.max(s)] {
                if *slot == u32::MAX {
                    *slot = i as u32;
                }
            }
        }
        if tri_part.contains(&u32::MAX) {
            let extra = names.len() as u32;
            names.push(if mesh.parts.is_empty() { fallback.to_string() } else { "unassigned".to_string() });
            materials.push(None);
            for slot in &mut tri_part {
                if *slot == u32::MAX {
                    *slot = extra;
                }
            }
        }
        let mut counts = vec![0usize; names.len()];
        for &p in &tri_part {
            counts[p as usize] += 1;
        }
        PartTable { names, materials, tri_part, counts }
    }

    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }
}
