// Builders writing small glTF models and clip libraries to disk for binding tests.
// See: context/lib/testing_guide.md §4

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};
use serde_json::{Value, json};

/// One document node, named by the rig it belongs to.
#[derive(Clone)]
pub(crate) struct Node {
    name: String,
    parent: Option<String>,
    translation: Vec3,
    rotation: [f32; 4],
    scale: Vec3,
}

pub(crate) fn node(name: &str) -> Node {
    Node {
        name: name.to_string(),
        parent: None,
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY.to_array(),
        scale: Vec3::ONE,
    }
}

impl Node {
    pub(crate) fn under(mut self, parent: &str) -> Self {
        self.parent = Some(parent.to_string());
        self
    }

    pub(crate) fn at(mut self, translation: Vec3) -> Self {
        self.translation = translation;
        self
    }

    pub(crate) fn turned(mut self, rotation: Quat) -> Self {
        self.rotation = rotation.to_array();
        self
    }

    /// A rest rotation that is no rotation at all.
    pub(crate) fn zero_rotation(mut self) -> Self {
        self.rotation = [0.0; 4];
        self
    }

    pub(crate) fn scaled(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }
}

struct Channel {
    node: String,
    path: &'static str,
    interpolation: &'static str,
    times: Vec<f32>,
    /// `times.len()` groups of 3 or 4 floats.
    values: Vec<f32>,
    value_type: &'static str,
}

/// One animation, its channels addressed by node name.
pub(crate) struct Clip {
    name: String,
    channels: Vec<Channel>,
}

pub(crate) fn clip(name: &str) -> Clip {
    Clip {
        name: name.to_string(),
        channels: Vec::new(),
    }
}

impl Clip {
    pub(crate) fn rotation(self, node: &str, times: &[f32], values: &[Quat]) -> Self {
        self.channel(
            node,
            "rotation",
            "LINEAR",
            "VEC4",
            times,
            values.iter().flat_map(|value| value.to_array()).collect(),
        )
    }

    pub(crate) fn stepped_rotation(self, node: &str, times: &[f32], values: &[Quat]) -> Self {
        self.channel(
            node,
            "rotation",
            "STEP",
            "VEC4",
            times,
            values.iter().flat_map(|value| value.to_array()).collect(),
        )
    }

    pub(crate) fn translation(self, node: &str, times: &[f32], values: &[Vec3]) -> Self {
        self.channel(
            node,
            "translation",
            "LINEAR",
            "VEC3",
            times,
            values.iter().flat_map(|value| value.to_array()).collect(),
        )
    }

    pub(crate) fn stepped_translation(self, node: &str, times: &[f32], values: &[Vec3]) -> Self {
        self.channel(
            node,
            "translation",
            "STEP",
            "VEC3",
            times,
            values.iter().flat_map(|value| value.to_array()).collect(),
        )
    }

    pub(crate) fn scale(self, node: &str, times: &[f32], values: &[Vec3]) -> Self {
        self.channel(
            node,
            "scale",
            "LINEAR",
            "VEC3",
            times,
            values.iter().flat_map(|value| value.to_array()).collect(),
        )
    }

    fn channel(
        mut self,
        node: &str,
        path: &'static str,
        interpolation: &'static str,
        value_type: &'static str,
        times: &[f32],
        values: Vec<f32>,
    ) -> Self {
        self.channels.push(Channel {
            node: node.to_string(),
            path,
            interpolation,
            times: times.to_vec(),
            values,
            value_type,
        });
        self
    }
}

enum Mesh {
    None,
    Static,
    Skinned {
        joints: Vec<String>,
        declared_root: Option<String>,
    },
}

/// A glTF document under construction.
pub(crate) struct Document {
    nodes: Vec<Node>,
    mesh: Mesh,
    clips: Vec<Clip>,
    extras: Option<Value>,
}

/// A mesh-less document: the shape of a clip library.
pub(crate) fn library(nodes: Vec<Node>) -> Document {
    Document {
        nodes,
        mesh: Mesh::None,
        clips: Vec::new(),
        extras: None,
    }
}

/// A one-triangle mesh skinned to `nodes`, in skin order, every vertex on the
/// first skin joint.
pub(crate) fn skinned(nodes: Vec<Node>) -> Document {
    let joints = nodes.iter().map(|node| node.name.clone()).collect();
    Document {
        nodes,
        mesh: Mesh::Skinned {
            joints,
            declared_root: None,
        },
        clips: Vec::new(),
        extras: None,
    }
}

/// A one-triangle mesh with no skin.
pub(crate) fn rigid() -> Document {
    Document {
        nodes: Vec::new(),
        mesh: Mesh::Static,
        clips: Vec::new(),
        extras: None,
    }
}

impl Document {
    pub(crate) fn clip(mut self, clip: Clip) -> Self {
        self.clips.push(clip);
        self
    }

    /// Declare the skin's `skeleton` node.
    pub(crate) fn declared_root(mut self, name: &str) -> Self {
        let Mesh::Skinned { declared_root, .. } = &mut self.mesh else {
            panic!("only a skinned fixture declares a root");
        };
        *declared_root = Some(name.to_string());
        self
    }

    /// Set the document-level `extras.clipLibrary` to a string.
    pub(crate) fn clip_library(self, reference: &str) -> Self {
        self.extras(json!({ "clipLibrary": reference }))
    }

    pub(crate) fn extras(mut self, extras: Value) -> Self {
        self.extras = Some(extras);
        self
    }

    /// Write `<path>` and, beside it, `<path>.bin`. Returns `path`.
    pub(crate) fn write(&self, path: &Path) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let buffer_name = format!("{}.bin", path.file_stem().unwrap().to_str().unwrap());
        let mut writer = BufferWriter::default();

        let mut indices: BTreeMap<&str, usize> = BTreeMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            indices.insert(&node.name, index);
        }
        let mut nodes: Vec<Value> = self.nodes.iter().map(node_json).collect();
        for node in &self.nodes {
            if let Some(parent) = &node.parent {
                let child = indices[node.name.as_str()];
                let children = nodes[indices[parent.as_str()]]
                    .as_object_mut()
                    .unwrap()
                    .entry("children")
                    .or_insert_with(|| json!([]));
                children.as_array_mut().unwrap().push(json!(child));
            }
        }
        let mut scene_nodes: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| node.parent.is_none().then_some(index))
            .collect();

        let mut root = json!({ "asset": { "version": "2.0" } });
        if !matches!(self.mesh, Mesh::None) {
            root["meshes"] = json!([{ "primitives": [writer.triangle(&self.mesh)] }]);
            let mut mesh_node = json!({ "name": "fixture_mesh", "mesh": 0 });
            if let Mesh::Skinned {
                joints,
                declared_root,
            } = &self.mesh
            {
                mesh_node["skin"] = json!(0);
                let joints: Vec<usize> = joints.iter().map(|name| indices[name.as_str()]).collect();
                root["skins"] = json!([{ "joints": joints }]);
                if let Some(name) = declared_root {
                    root["skins"][0]["skeleton"] = json!(indices[name.as_str()]);
                }
            }
            scene_nodes.push(nodes.len());
            nodes.push(mesh_node);
        }

        if !self.clips.is_empty() {
            let animations: Vec<Value> = self
                .clips
                .iter()
                .map(|clip| writer.animation(clip, &indices))
                .collect();
            root["animations"] = json!(animations);
        }
        root["nodes"] = json!(nodes);
        root["scenes"] = json!([{ "nodes": scene_nodes }]);
        root["scene"] = json!(0);
        if let Some(extras) = &self.extras {
            root["extras"] = extras.clone();
        }
        if !writer.bytes.is_empty() {
            root["buffers"] = json!([{ "uri": buffer_name, "byteLength": writer.bytes.len() }]);
            root["bufferViews"] = json!(writer.views);
            root["accessors"] = json!(writer.accessors);
            std::fs::write(path.with_file_name(&buffer_name), &writer.bytes).unwrap();
        }
        std::fs::write(path, serde_json::to_vec_pretty(&root).unwrap()).unwrap();
        path.to_path_buf()
    }
}

fn node_json(node: &Node) -> Value {
    json!({
        "name": node.name,
        "translation": node.translation.to_array(),
        "rotation": node.rotation,
        "scale": node.scale.to_array(),
    })
}

#[derive(Default)]
struct BufferWriter {
    bytes: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl BufferWriter {
    fn accessor(&mut self, bytes: &[u8], mut accessor: Value) -> usize {
        self.views.push(json!({
            "buffer": 0,
            "byteOffset": self.bytes.len(),
            "byteLength": bytes.len(),
        }));
        self.bytes.extend_from_slice(bytes);
        accessor["bufferView"] = json!(self.views.len() - 1);
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    fn floats(&mut self, values: &[f32], accessor_type: &str, count: usize) -> usize {
        let bytes: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        self.accessor(
            &bytes,
            json!({ "componentType": 5126, "count": count, "type": accessor_type }),
        )
    }

    fn triangle(&mut self, mesh: &Mesh) -> Value {
        let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let position = self.floats(&positions, "VEC3", 3);
        self.accessors[position]["min"] = json!([0.0, 0.0, 0.0]);
        self.accessors[position]["max"] = json!([1.0, 1.0, 0.0]);
        let mut attributes = json!({ "POSITION": position });
        if matches!(mesh, Mesh::Skinned { .. }) {
            attributes["JOINTS_0"] = json!(self.accessor(
                &[0; 12],
                json!({ "componentType": 5121, "count": 3, "type": "VEC4" }),
            ));
            let weights = [1.0, 0.0, 0.0, 0.0].repeat(3);
            attributes["WEIGHTS_0"] = json!(self.floats(&weights, "VEC4", 3));
        }
        json!({ "attributes": attributes, "mode": 4 })
    }

    fn animation(&mut self, clip: &Clip, nodes: &BTreeMap<&str, usize>) -> Value {
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for channel in &clip.channels {
            let count = channel.times.len();
            let input = self.floats(&channel.times, "SCALAR", count);
            let output = self.floats(&channel.values, channel.value_type, count);
            channels.push(json!({
                "sampler": samplers.len(),
                "target": { "node": nodes[channel.node.as_str()], "path": channel.path },
            }));
            samplers.push(json!({
                "input": input,
                "output": output,
                "interpolation": channel.interpolation,
            }));
        }
        json!({ "name": clip.name, "samplers": samplers, "channels": channels })
    }
}
