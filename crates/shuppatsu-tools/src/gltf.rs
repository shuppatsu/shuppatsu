//! Minimal glTF 2.0 writer (JSON + external .bin), hand-rolled to stay within the dependency
//! allowlist. Only what the exports need: indexed triangle meshes, line strips, textured
//! materials with nearest filtering and repeat wrapping, and a flat node list.

use std::fmt::Write as _;

#[derive(Default)]
pub struct Gltf {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
    images: Vec<String>,
    materials: Vec<String>,
    meshes: Vec<String>,
    nodes: Vec<String>,
}

/// One primitive to add to a mesh.
pub struct Primitive {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Option<Vec<[f32; 2]>>,
    pub indices: Option<Vec<u32>>,
    pub material: Option<usize>,
    /// glTF primitive mode: 4 = triangles, 3 = line strip.
    pub mode: u32,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn num(v: f32) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".into()
    }
}

impl Gltf {
    fn push_view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let target = target.map_or(String::new(), |t| format!(r#","target":{t}"#));
        self.views.push(format!(
            r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{}{target}}}"#,
            bytes.len()
        ));
        self.views.len() - 1
    }

    fn vec_accessor<const N: usize>(&mut self, data: &[[f32; N]], with_bounds: bool) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let view = self.push_view(&bytes, Some(34962));
        let kind = if N == 3 { "VEC3" } else { "VEC2" };
        let bounds = if with_bounds && !data.is_empty() {
            let mut min = [f32::MAX; N];
            let mut max = [f32::MIN; N];
            for v in data {
                for i in 0..N {
                    min[i] = min[i].min(v[i]);
                    max[i] = max[i].max(v[i]);
                }
            }
            let list = |a: [f32; N]| a.iter().map(|&v| num(v)).collect::<Vec<_>>().join(",");
            format!(r#","min":[{}],"max":[{}]"#, list(min), list(max))
        } else {
            String::new()
        };
        self.accessors.push(format!(
            r#"{{"bufferView":{view},"componentType":5126,"count":{},"type":"{kind}"{bounds}}}"#,
            data.len()
        ));
        self.accessors.len() - 1
    }

    fn index_accessor(&mut self, idx: &[u32]) -> usize {
        let bytes: Vec<u8> = idx.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.push_view(&bytes, Some(34963));
        self.accessors.push(format!(
            r#"{{"bufferView":{view},"componentType":5125,"count":{},"type":"SCALAR"}}"#,
            idx.len()
        ));
        self.accessors.len() - 1
    }

    /// A texture-mapped material using an image file (relative URI), alpha-masked so the
    /// colour-keyed (fully transparent) texels are cut out.
    pub fn textured_material(&mut self, name: &str, uri: &str) -> usize {
        self.images.push(format!(r#"{{"uri":"{}"}}"#, esc(uri)));
        let image = self.images.len() - 1;
        self.materials.push(format!(
            r#"{{"name":"{}","pbrMetallicRoughness":{{"baseColorTexture":{{"index":{image}}},"metallicFactor":0,"roughnessFactor":1}},"alphaMode":"MASK","alphaCutoff":0.5,"doubleSided":false}}"#,
            esc(name)
        ));
        self.materials.len() - 1
    }

    pub fn colour_material(&mut self, name: &str, rgb: [f32; 3]) -> usize {
        self.materials.push(format!(
            r#"{{"name":"{}","pbrMetallicRoughness":{{"baseColorFactor":[{},{},{},1],"metallicFactor":0,"roughnessFactor":1}}}}"#,
            esc(name),
            num(rgb[0]),
            num(rgb[1]),
            num(rgb[2])
        ));
        self.materials.len() - 1
    }

    pub fn mesh(&mut self, name: &str, prims: Vec<Primitive>) -> usize {
        let mut out = Vec::new();
        for p in prims {
            let pos = self.vec_accessor(&p.positions, true);
            let mut attrs = format!(r#""POSITION":{pos}"#);
            if let Some(uv) = &p.uvs {
                let a = self.vec_accessor(uv, false);
                let _ = write!(attrs, r#","TEXCOORD_0":{a}"#);
            }
            let mut s = format!(r#"{{"attributes":{{{attrs}}},"mode":{}"#, p.mode);
            if let Some(idx) = &p.indices {
                let a = self.index_accessor(idx);
                let _ = write!(s, r#","indices":{a}"#);
            }
            if let Some(m) = p.material {
                let _ = write!(s, r#","material":{m}"#);
            }
            s.push('}');
            out.push(s);
        }
        self.meshes.push(format!(
            r#"{{"name":"{}","primitives":[{}]}}"#,
            esc(name),
            out.join(",")
        ));
        self.meshes.len() - 1
    }

    /// A node with translation and a rotation about +Y (radians).
    pub fn node(&mut self, name: &str, mesh: Option<usize>, t: [f32; 3], yaw: f32) {
        let mesh = mesh.map_or(String::new(), |m| format!(r#","mesh":{m}"#));
        let (s, c) = (yaw * 0.5).sin_cos();
        self.nodes.push(format!(
            r#"{{"name":"{}"{mesh},"translation":[{},{},{}],"rotation":[0,{},0,{}]}}"#,
            esc(name),
            num(t[0]),
            num(t[1]),
            num(t[2]),
            num(s),
            num(c)
        ));
    }

    /// Returns (glTF JSON, binary buffer). `bin_uri` is the buffer's file name.
    pub fn finish(self, bin_uri: &str) -> (String, Vec<u8>) {
        let textures: Vec<String> = (0..self.images.len())
            .map(|i| format!(r#"{{"sampler":0,"source":{i}}}"#))
            .collect();
        let scene_nodes: Vec<String> = (0..self.nodes.len()).map(|i| i.to_string()).collect();
        let json = format!(
            concat!(
                r#"{{"asset":{{"version":"2.0","generator":"shuppatsu-tools"}},"#,
                r#""scene":0,"scenes":[{{"nodes":[{}]}}],"nodes":[{}],"meshes":[{}],"#,
                r#""materials":[{}],"textures":[{}],"images":[{}],"#,
                // Nearest filtering and repeat wrap, as the original (no filter states set, D3D
                // default WRAP addressing).
                r#""samplers":[{{"magFilter":9728,"minFilter":9728,"wrapS":10497,"wrapT":10497}}],"#,
                r#""accessors":[{}],"bufferViews":[{}],"#,
                r#""buffers":[{{"uri":"{}","byteLength":{}}}]}}"#
            ),
            scene_nodes.join(","),
            self.nodes.join(","),
            self.meshes.join(","),
            self.materials.join(","),
            textures.join(","),
            self.images.join(","),
            self.accessors.join(","),
            self.views.join(","),
            esc(bin_uri),
            self.bin.len()
        );
        (json, self.bin)
    }
}
