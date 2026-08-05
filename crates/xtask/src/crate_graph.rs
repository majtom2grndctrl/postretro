//! `crate-graph`: internal crate dependency analysis.
//!
//! Reads `cargo metadata` and reduces it to the workspace's *internal* crate
//! graph — normal (non-dev, non-build) edges between workspace members, with
//! external crates omitted by design. From that one deterministic reduction it
//! serves three jobs, all documented in `context/lib/development_guide.md`:
//!
//! - **view** (`crate-graph`, or `--mermaid` for the on-demand edge diagram);
//! - **gate** (`crate-graph --check`): fail if the committed
//!   `context/lib/crate-graph.md` is stale, the `cargo fmt --check` pattern;
//! - **query** (`crate-graph --rdeps/--deps <crate>`): live blast-radius /
//!   forward-dependency questions.
//!
//! The committed snapshot holds only the layers and chokepoint ranking — the
//! cheap, diff-stable views. The full edge diagram is generated on demand
//! (`--mermaid`) rather than committed, so no dense graph churns the diff.
//!
//! The layering *invariants* are enforced separately as a `#[test]` (see the
//! bottom of this file), so `cargo test` in preflight catches an upward edge or
//! a chokepoint-widening dependency even when the committed doc is fresh.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// Repo-relative path of the committed snapshot the `--check` gate guards.
const DOC_PATH: &str = "context/lib/crate-graph.md";

/// The internal crate graph: workspace members and their normal internal edges.
///
/// `crates` holds every workspace member's full package name, sorted. `edges`
/// holds `(dependent, dependency)` pairs — both full names, both members —
/// sorted and deduplicated. Everything downstream is derived from these two
/// sorted collections, so every rendered artifact is deterministic.
#[derive(Debug, PartialEq, Eq)]
pub struct Graph {
    crates: Vec<String>,
    edges: Vec<(String, String)>,
}

pub fn run(args: Vec<OsString>) -> Result<i32, String> {
    let workspace_root = crate::workspace_root()?;
    let strings: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    match strings.as_slice() {
        [] => {
            let graph = load_graph(&workspace_root)?;
            print!("{}", render_doc(&graph));
            Ok(0)
        }
        [flag] if flag == "--write" => {
            let graph = load_graph(&workspace_root)?;
            let path = workspace_root.join(DOC_PATH);
            std::fs::write(&path, render_doc(&graph))
                .map_err(|e| format!("write {}: {e}", path.display()))?;
            println!("Wrote {DOC_PATH}");
            Ok(0)
        }
        [flag] if flag == "--check" => {
            let graph = load_graph(&workspace_root)?;
            check_committed_doc(&workspace_root, &graph)
        }
        [flag] if flag == "--mermaid" => {
            let graph = load_graph(&workspace_root)?;
            print!("{}", render_mermaid(&graph));
            Ok(0)
        }
        [flag, name] if flag == "--rdeps" => {
            let graph = load_graph(&workspace_root)?;
            print_query(&graph, name, Direction::Dependents)
        }
        [flag, name] if flag == "--deps" => {
            let graph = load_graph(&workspace_root)?;
            print_query(&graph, name, Direction::Dependencies)
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "crate-graph usage:\n  \
       cargo run -p xtask -- crate-graph                  Print the layers + chokepoint ranking\n  \
       cargo run -p xtask -- crate-graph --write          Regenerate context/lib/crate-graph.md\n  \
       cargo run -p xtask -- crate-graph --check          Fail if the committed doc is stale\n  \
       cargo run -p xtask -- crate-graph --mermaid        Print the full edge diagram (Mermaid)\n  \
       cargo run -p xtask -- crate-graph --rdeps <crate>  What depends on <crate> (blast radius)\n  \
       cargo run -p xtask -- crate-graph --deps <crate>   What <crate> depends on"
        .to_string()
}

// --- Loading --------------------------------------------------------------

/// Run `cargo metadata` at the workspace root and reduce it to the internal
/// graph. `--no-deps` restricts the package list to workspace members, so the
/// member set *is* the "internal" set — no allow-list to maintain.
fn load_graph(workspace_root: &Path) -> Result<Graph, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(&cargo)
        .current_dir(workspace_root)
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .output()
        .map_err(|e| format!("run cargo metadata: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "cargo metadata exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let json = String::from_utf8(output.stdout)
        .map_err(|e| format!("cargo metadata emitted non-UTF-8: {e}"))?;
    collect_graph(&json)
}

/// Reduce raw `cargo metadata` JSON to the internal graph. Kept pure (takes the
/// JSON string, shells nothing) so it is unit-testable against a fixture.
fn collect_graph(metadata_json: &str) -> Result<Graph, String> {
    let value: Value =
        serde_json::from_str(metadata_json).map_err(|e| format!("parse cargo metadata: {e}"))?;
    let packages = value["packages"]
        .as_array()
        .ok_or("cargo metadata: missing `packages` array")?;

    let internal: BTreeSet<String> = packages
        .iter()
        .filter_map(|pkg| pkg["name"].as_str().map(str::to_string))
        .collect();

    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();
    for pkg in packages {
        let Some(name) = pkg["name"].as_str() else {
            continue;
        };
        let Some(deps) = pkg["dependencies"].as_array() else {
            continue;
        };
        for dep in deps {
            // `kind` is null for a normal dependency, "dev" or "build"
            // otherwise. Only normal edges shape the compile graph we care
            // about, so dev-dependencies and build-dependencies are dropped.
            let is_normal = dep.get("kind").map(Value::is_null).unwrap_or(false);
            if !is_normal {
                continue;
            }
            let Some(dep_name) = dep["name"].as_str() else {
                continue;
            };
            if dep_name != name && internal.contains(dep_name) {
                edges.insert((name.to_string(), dep_name.to_string()));
            }
        }
    }

    Ok(Graph {
        crates: internal.into_iter().collect(),
        edges: edges.into_iter().collect(),
    })
}

// --- Derived views --------------------------------------------------------

/// Strip the `postretro-` prefix for display; the binary (`postretro`) and
/// `xtask` carry no prefix and pass through unchanged.
fn short(name: &str) -> &str {
    name.strip_prefix("postretro-").unwrap_or(name)
}

/// A Mermaid-safe node id: the short name with hyphens folded to underscores.
fn node_id(name: &str) -> String {
    short(name).replace('-', "_")
}

impl Graph {
    /// Longest-path depth of each crate from the leaves: a crate sits one layer
    /// above its deepest internal dependency. Computed by relaxation to a
    /// fixpoint — correct for any DAG, and cargo guarantees the normal-edge
    /// graph is acyclic.
    fn ranks(&self) -> BTreeMap<String, usize> {
        let mut rank: BTreeMap<String, usize> =
            self.crates.iter().map(|c| (c.clone(), 0)).collect();
        loop {
            let mut changed = false;
            for (from, to) in &self.edges {
                let candidate = rank[to] + 1;
                if candidate > rank[from] {
                    rank.insert(from.clone(), candidate);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        rank
    }

    /// Crates grouped by rank, ascending; each group sorted by full name.
    fn layers(&self) -> Vec<Vec<String>> {
        let rank = self.ranks();
        let depth = rank.values().copied().max().unwrap_or(0);
        let mut layers = vec![Vec::new(); depth + 1];
        for crate_name in &self.crates {
            layers[rank[crate_name]].push(crate_name.clone());
        }
        layers
    }

    /// Direct dependents of each crate (the crates with an edge *into* it),
    /// sorted by dependent count descending, then name — the compile-chokepoint
    /// ranking. Crates nothing depends on are omitted.
    fn dependents_ranking(&self) -> Vec<(String, Vec<String>)> {
        let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (from, to) in &self.edges {
            dependents.entry(to.clone()).or_default().push(from.clone());
        }
        let mut ranked: Vec<(String, Vec<String>)> = dependents.into_iter().collect();
        ranked.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        ranked
    }

    /// Transitive reachable set from `start` following edges in `direction`,
    /// excluding `start` itself. Sorted full names.
    fn reachable(&self, start: &str, direction: Direction) -> Vec<String> {
        let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (from, to) in &self.edges {
            let (key, value) = match direction {
                Direction::Dependencies => (from.as_str(), to.as_str()),
                Direction::Dependents => (to.as_str(), from.as_str()),
            };
            adjacency.entry(key).or_default().push(value);
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<String> = VecDeque::from([start.to_string()]);
        while let Some(node) = queue.pop_front() {
            for &next in adjacency.get(node.as_str()).into_iter().flatten() {
                if seen.insert(next.to_string()) {
                    queue.push_back(next.to_string());
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Resolve a user-supplied crate name, accepting either the full package
    /// name or the short (prefix-stripped) form.
    fn resolve(&self, name: &str) -> Option<String> {
        if self.crates.iter().any(|c| c == name) {
            return Some(name.to_string());
        }
        self.crates.iter().find(|c| short(c) == name).cloned()
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Dependents,
    Dependencies,
}

// --- Rendering ------------------------------------------------------------

fn render_mermaid(graph: &Graph) -> String {
    let mut out = String::from("graph TD\n");
    for crate_name in &graph.crates {
        out.push_str(&format!(
            "    {}[\"{}\"]\n",
            node_id(crate_name),
            short(crate_name)
        ));
    }
    for (from, to) in &graph.edges {
        out.push_str(&format!("    {} --> {}\n", node_id(from), node_id(to)));
    }
    out
}

/// The committed snapshot: header, computed layers, and the chokepoint ranking
/// (the full edge diagram is generated on demand by `--mermaid`, not committed).
/// Generated verbatim by `--write` and byte-compared by `--check`, so the format
/// must stay deterministic.
fn render_doc(graph: &Graph) -> String {
    let mut out = String::new();
    out.push_str(
        "<!-- GENERATED by `cargo run -p xtask -- crate-graph --write`. Do not edit by hand. -->\n\
         <!-- Regenerate whenever a crate's internal (non-dev) dependencies change; \
         `crate-graph --check` gates staleness in preflight. -->\n\n\
         # Crate graph\n\n\
         Internal workspace crates, grouped by layer and ranked by how many crates\n\
         depend on them. External crates are omitted by design. A crate's layer is\n\
         one above its deepest internal dependency. See the layering invariants in\n\
         `development_guide.md` §Workspace.\n\n\
         Generate the full edge diagram on demand with `cargo run -p xtask -- \
         crate-graph --mermaid`; query a crate's blast radius with `--rdeps <crate>` \
         or its dependencies with `--deps <crate>`.\n\n\
         ## Layers\n\n",
    );
    for (rank, members) in graph.layers().iter().enumerate() {
        let label = if rank == 0 { " (leaves)" } else { "" };
        let names: Vec<&str> = members.iter().map(|m| short(m)).collect();
        out.push_str(&format!(
            "- **Layer {rank}{label}:** {}\n",
            names.join(", ")
        ));
    }
    out.push_str(
        "\n## Dependents\n\n\
         Crates ranked by how many workspace crates depend on them directly —\n\
         the compile chokepoints. Changing a public type in a high-ranked crate\n\
         recompiles every dependent.\n\n",
    );
    for (crate_name, dependents) in graph.dependents_ranking() {
        let names: Vec<&str> = dependents.iter().map(|d| short(d)).collect();
        out.push_str(&format!(
            "- **{}** — {} dependent{} ({})\n",
            short(&crate_name),
            dependents.len(),
            if dependents.len() == 1 { "" } else { "s" },
            names.join(", ")
        ));
    }
    out
}

// --- Modes ----------------------------------------------------------------

fn check_committed_doc(workspace_root: &Path, graph: &Graph) -> Result<i32, String> {
    let path = workspace_root.join(DOC_PATH);
    let expected = render_doc(graph);
    let actual = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "read {}: {e}\n  run `cargo run -p xtask -- crate-graph --write` to create it",
            path.display()
        )
    })?;

    if actual == expected {
        println!("crate-graph: {DOC_PATH} is up to date");
        Ok(0)
    } else {
        eprintln!(
            "crate-graph: {DOC_PATH} is stale.\n  \
             The internal crate graph changed but the committed snapshot was not\n  \
             regenerated. Run `cargo run -p xtask -- crate-graph --write` and commit."
        );
        Ok(1)
    }
}

fn print_query(graph: &Graph, name: &str, direction: Direction) -> Result<i32, String> {
    let resolved = graph.resolve(name).ok_or_else(|| {
        format!(
            "unknown crate `{name}` — not a workspace member (try a short name like `entities`)"
        )
    })?;
    let reachable = graph.reachable(&resolved, direction);
    let target = short(&resolved);

    if reachable.is_empty() {
        match direction {
            Direction::Dependents => println!("Nothing depends on {target}."),
            Direction::Dependencies => println!("{target} depends on no internal crate."),
        }
        return Ok(0);
    }

    match direction {
        Direction::Dependents => println!(
            "{} crates depend on {target} (transitively):",
            reachable.len()
        ),
        Direction::Dependencies => println!(
            "{target} depends on {} crates (transitively):",
            reachable.len()
        ),
    }
    for crate_name in &reachable {
        println!("  {}", short(crate_name));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `cargo metadata --no-deps` shape: three members, one normal
    /// edge, one dev edge (must be dropped), one external dep (must be dropped).
    fn fixture() -> &'static str {
        r#"{
          "packages": [
            {
              "name": "postretro-foundation",
              "dependencies": [
                { "name": "glam", "kind": null }
              ]
            },
            {
              "name": "postretro-entities",
              "dependencies": [
                { "name": "postretro-foundation", "kind": null },
                { "name": "serde_json", "kind": null },
                { "name": "postretro-foundation", "kind": "dev" }
              ]
            },
            {
              "name": "postretro",
              "dependencies": [
                { "name": "postretro-entities", "kind": null }
              ]
            }
          ]
        }"#
    }

    #[test]
    fn collect_graph_keeps_only_internal_normal_edges() {
        let graph = collect_graph(fixture()).expect("fixture parses");
        assert_eq!(
            graph.crates,
            vec![
                "postretro".to_string(),
                "postretro-entities".to_string(),
                "postretro-foundation".to_string(),
            ]
        );
        // glam (external) and serde_json (external) dropped; the dev edge on
        // entities dropped; only the two internal normal edges remain.
        assert_eq!(
            graph.edges,
            vec![
                ("postretro".to_string(), "postretro-entities".to_string()),
                (
                    "postretro-entities".to_string(),
                    "postretro-foundation".to_string()
                ),
            ]
        );
    }

    #[test]
    fn ranks_place_each_crate_above_its_deepest_dependency() {
        let graph = collect_graph(fixture()).expect("fixture parses");
        let ranks = graph.ranks();
        assert_eq!(ranks["postretro-foundation"], 0);
        assert_eq!(ranks["postretro-entities"], 1);
        assert_eq!(ranks["postretro"], 2);
    }

    #[test]
    fn reachable_walks_dependents_and_dependencies_transitively() {
        let graph = collect_graph(fixture()).expect("fixture parses");
        assert_eq!(
            graph.reachable("postretro-foundation", Direction::Dependents),
            vec!["postretro".to_string(), "postretro-entities".to_string()]
        );
        assert_eq!(
            graph.reachable("postretro", Direction::Dependencies),
            vec![
                "postretro-entities".to_string(),
                "postretro-foundation".to_string()
            ]
        );
    }

    #[test]
    fn resolve_accepts_full_and_short_names() {
        let graph = collect_graph(fixture()).expect("fixture parses");
        assert_eq!(
            graph.resolve("entities"),
            Some("postretro-entities".to_string())
        );
        assert_eq!(
            graph.resolve("postretro-entities"),
            Some("postretro-entities".to_string())
        );
        assert_eq!(graph.resolve("nonexistent"), None);
    }

    /// Layering invariants from `development_guide.md` §Workspace, enforced
    /// against the *live* workspace graph so an upward edge or a widened
    /// chokepoint fails preflight `cargo test`. This shells `cargo metadata`;
    /// the pure-logic tests above cover parsing without it.
    #[test]
    fn layering_invariants_hold() {
        let workspace_root = crate::workspace_root().expect("workspace root");
        let graph = load_graph(&workspace_root).expect("load live crate graph");

        // 1. Nothing depends on the binary — the top of the one-way graph.
        let binary_dependents = graph.reachable("postretro", Direction::Dependents);
        assert!(
            binary_dependents.is_empty(),
            "the `postretro` binary must have no dependents, found: {binary_dependents:?}"
        );

        // 2. `foundation` is a base leaf — no internal dependencies.
        let foundation_deps = graph.reachable("postretro-foundation", Direction::Dependencies);
        assert!(
            foundation_deps.is_empty(),
            "`foundation` must be a leaf, but depends on: {foundation_deps:?}"
        );

        // 3. The `entities` chokepoint stays thin: `foundation` is its only
        //    internal dependency. Domain logic accreting here would recompile
        //    the whole downstream render/UI stack.
        let entities_deps: Vec<String> = graph
            .edges
            .iter()
            .filter(|(from, _)| from == "postretro-entities")
            .map(|(_, to)| to.clone())
            .collect();
        assert_eq!(
            entities_deps,
            vec!["postretro-foundation".to_string()],
            "`entities` must depend only on `foundation` to stay a thin chokepoint"
        );

        // 4. The transport stays an engine/registry-blind leaf. Wire identities
        // are plain values; it must never learn an internal engine crate merely
        // to carry them.
        let net_deps = graph.reachable("postretro-net", Direction::Dependencies);
        assert!(
            net_deps.is_empty(),
            "`postretro-net` must have no internal dependencies, found: {net_deps:?}"
        );
    }
}
