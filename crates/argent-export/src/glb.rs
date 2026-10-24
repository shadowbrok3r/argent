//! Minimal glTF 2.0 binary (GLB) writer: one buffer, one buffer view per
//! attribute, one mesh + node per primitive under a root node.

use serde_json::{Value, json};

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const TRIANGLES: u32 = 4;
const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"

/// One part, ready to write: positions in metres, unit normals.
pub(crate) struct Prim {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Option<Vec<[f32; 3]>>,
    pub indices: Vec<u32>,
    pub material: usize,
}

struct Builder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Builder {
    fn view(&mut self, data: Vec<u8>, target: u32) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len(), "target": target }));
        self.bin.extend_from_slice(&data);
        self.views.len() - 1
    }

    fn vec3(&mut self, data: &[[f32; 3]], bounds: bool) -> usize {
        let mut bytes = Vec::with_capacity(data.len() * 12);
        for v in data {
            for x in v {
                bytes.extend_from_slice(&x.to_le_bytes());
            }
        }
        let view = self.view(bytes, ARRAY_BUFFER);
        let mut acc = json!({ "bufferView": view, "componentType": FLOAT, "count": data.len(), "type": "VEC3" });
        if bounds {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for v in data {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            // As f64 so the JSON holds the exact f32 values the validator compares.
            acc["min"] = json!(lo.map(|x| x as f64));
            acc["max"] = json!(hi.map(|x| x as f64));
        }
        self.accessors.push(acc);
        self.accessors.len() - 1
    }

    fn indices(&mut self, data: &[u32]) -> usize {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for i in data {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        let view = self.view(bytes, ELEMENT_ARRAY_BUFFER);
        self.accessors
            .push(json!({ "bufferView": view, "componentType": UNSIGNED_INT, "count": data.len(), "type": "SCALAR" }));
        self.accessors.len() - 1
    }
}

/// Encode a GLB. `prims` must be non-empty and every primitive non-empty;
/// `extensions` lists the material extensions in use.
pub(crate) fn encode(scene: &str, prims: &[Prim], materials: &[Value], extensions: &[&str]) -> Vec<u8> {
    let mut b = Builder { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let mut meshes = Vec::with_capacity(prims.len());
    let mut nodes = Vec::with_capacity(prims.len() + 1);
    nodes.push(Value::Null); // root, filled below
    for (i, p) in prims.iter().enumerate() {
        let pos = b.vec3(&p.positions, true);
        let nrm = b.vec3(&p.normals, false);
        let mut attributes = json!({ "POSITION": pos, "NORMAL": nrm });
        if let Some(c) = &p.colors {
            attributes["COLOR_0"] = json!(b.vec3(c, false));
        }
        let idx = b.indices(&p.indices);
        meshes.push(json!({
            "name": p.name,
            "primitives": [{ "attributes": attributes, "indices": idx, "material": p.material, "mode": TRIANGLES }],
        }));
        nodes.push(json!({ "name": p.name, "mesh": i }));
    }
    nodes[0] = json!({ "name": scene, "children": (1..=prims.len()).collect::<Vec<_>>() });
    while !b.bin.len().is_multiple_of(4) {
        b.bin.push(0);
    }
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": concat!("argent-export ", env!("CARGO_PKG_VERSION")) },
        "scene": 0,
        "scenes": [{ "name": scene, "nodes": [0] }],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": b.accessors,
        "bufferViews": b.views,
        "buffers": [{ "byteLength": b.bin.len() }],
    });
    if !extensions.is_empty() {
        doc["extensionsUsed"] = json!(extensions);
    }
    let mut js = serde_json::to_vec(&doc).expect("glTF JSON serialises");
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let total = 12 + 8 + js.len() + 8 + b.bin.len();
    let mut out = Vec::with_capacity(total);
    for w in [GLB_MAGIC, 2, total as u32, js.len() as u32, CHUNK_JSON] {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&js);
    out.extend_from_slice(&(b.bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(&b.bin);
    out
}
