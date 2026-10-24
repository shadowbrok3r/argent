//! File readers. TODO(agent): STL binary+ASCII, OBJ (v/vn ignored → recomputed,
//! f with any of v, v/vt, v//vn, v/vt/vn, negative indices, polygons fanned,
//! g/o/usemtl → parts), PLY (ascii, binary_little_endian, binary_big_endian;
//! vertex x/y/z float|double, face list uchar|int + int|uint), 3MF (zip,
//! 3D/3dmodel.model, objects → parts, build item transforms, unit attribute →
//! mm). Then weld + normals per `LoadOptions`.

use crate::{LoadOptions, Mesh};
use std::path::Path;

pub fn load(path: &Path, _opts: &LoadOptions) -> anyhow::Result<Mesh> {
    anyhow::bail!("argent-mesh: loader not implemented yet for {}", path.display())
}
