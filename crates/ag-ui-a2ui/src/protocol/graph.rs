//! The child-reference graph of a component list, and the one walk over it.
//!
//! Components are a flat list; the tree is what their child references say.
//! Three questions are asked of that graph — which components can the root
//! reach, does following references ever come back to where it started, and
//! how deep does it go — and they are asked of a finished surface and of one
//! that is still arriving alike. [`Graph::walk`] answers all three in one
//! depth-first pass, so there is one answer to what a cycle is and to which
//! components are attached.
//!
//! # Depth
//!
//! The walk keeps an explicit stack rather than recursing. The graph is
//! model-generated and its depth is bounded by nothing, so a recursive walk
//! would abort the process on a deep enough tree instead of failing a request.
//! `max_depth` is therefore a policy about what a renderer will draw, applied
//! by the caller that has one, and not what keeps the walk standing.

use std::collections::{BTreeMap, BTreeSet};

use super::catalog::ComponentRef;
use super::message::Component;

/// One child reference that resolved to a component in the list.
#[derive(Debug, Clone)]
pub(crate) struct Edge {
    /// Index of the referenced component.
    pub(crate) target: usize,
    /// Where the reference sits within the referring component, e.g.
    /// `children[1]`.
    pub(crate) location: String,
}

/// Child references between the components of one list, by index.
#[derive(Debug, Clone, Default)]
pub(crate) struct Graph {
    edges: Vec<Vec<Edge>>,
}

/// A loop the walk closed: the nodes on it, and the edge that closed it.
#[derive(Debug, Clone)]
pub(crate) struct Cycle {
    /// The path from the node the closing edge points at, down to the node
    /// the edge leaves. One node for a self-reference.
    pub(crate) nodes: Vec<usize>,
    /// The node whose reference closes the loop.
    pub(crate) from: usize,
    /// That reference.
    pub(crate) edge: Edge,
}

/// Something the walk found that a caller may want to report, in the order it
/// was found.
#[derive(Debug, Clone)]
pub(crate) enum Finding {
    /// Child references form a loop. Each distinct loop is found once.
    Cycle(Cycle),
    /// A reference leads deeper than `max_depth`. Found at most once per
    /// walk; the subtree behind it is not explored.
    TooDeep {
        /// The node whose reference crosses the limit.
        from: usize,
        /// That reference.
        edge: Edge,
    },
}

/// What one walk saw.
#[derive(Debug, Clone, Default)]
pub(crate) struct Walk {
    /// The nodes the walk entered: every start, and everything reachable from
    /// one without crossing `max_depth`.
    pub(crate) reached: BTreeSet<usize>,
    /// Loops and depth overruns, in discovery order.
    pub(crate) findings: Vec<Finding>,
}

impl Graph {
    /// Reads every component's child references with `references`, which is
    /// given the node and its component: the catalog a component is drawn
    /// from says which of its properties name children, and a surface may mix
    /// catalogs.
    ///
    /// `components` yields one entry per node, `None` for a node that is not a
    /// usable component (no id or type) and so refers to nothing. `ids` maps a
    /// component id to its node. A reference to an id that is not in `ids`
    /// becomes no edge; it is handed to `dangling` instead, with the node that
    /// made it.
    pub(crate) fn build<'a>(
        references: impl Fn(usize, &Component) -> Vec<ComponentRef>,
        components: impl IntoIterator<Item = Option<&'a Component>>,
        ids: &BTreeMap<&str, usize>,
        mut dangling: impl FnMut(usize, &Component, ComponentRef),
    ) -> Self {
        let edges = components
            .into_iter()
            .enumerate()
            .map(|(node, component)| {
                let Some(component) = component else {
                    return Vec::new();
                };
                let mut edges = Vec::new();
                for reference in references(node, component) {
                    match ids.get(reference.id.as_str()) {
                        Some(&target) => edges.push(Edge {
                            target,
                            location: reference.location,
                        }),
                        None => dangling(node, component, reference),
                    }
                }
                edges
            })
            .collect();
        Self { edges }
    }

    /// How many nodes the graph has.
    pub(crate) fn len(&self) -> usize {
        self.edges.len()
    }

    /// The references a node makes, in the order the catalog reports them.
    pub(crate) fn edges(&self, node: usize) -> &[Edge] {
        &self.edges[node]
    }

    /// Every reference in the graph.
    pub(crate) fn all_edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter().flatten()
    }

    /// Depth-first from each of `starts` in turn, entering every node at most
    /// once across the whole walk.
    ///
    /// A back edge to a node still on the current path closes a cycle; a
    /// self-reference is the one-node case of the same thing. Each distinct
    /// cycle is reported once, however many starts lead into it.
    ///
    /// Depth is measured along the search path, so it is the depth of the
    /// first route the walk finds to a node rather than the longest possible
    /// one. That matches every other A2UI toolkit, and finding true longest
    /// paths in a general graph is not something a validator should be doing.
    /// With `max_depth`, a reference that would enter a node deeper than the
    /// limit is reported once and the subtree behind it is left unexplored: it
    /// is condemned, and a pathological payload should not cost more work.
    pub(crate) fn walk(
        &self,
        starts: impl IntoIterator<Item = usize>,
        max_depth: Option<usize>,
    ) -> Walk {
        self.walk_from(starts.into_iter().map(|start| (start, max_depth)))
    }

    /// [`Graph::walk`], with a depth limit of its own for each start.
    ///
    /// Depth is measured from the start a node is first entered from, so the
    /// depth a renderer draws to is measured by putting its root first with
    /// the limit. The starts after it, with none, then find the loops the
    /// root does not reach, and a long loop among them is a loop and not a
    /// depth overrun.
    pub(crate) fn walk_from(
        &self,
        starts: impl IntoIterator<Item = (usize, Option<usize>)>,
    ) -> Walk {
        const WHITE: u8 = 0;
        const GRAY: u8 = 1;
        const BLACK: u8 = 2;

        let mut color = vec![WHITE; self.len()];
        let mut reported: BTreeSet<Vec<usize>> = BTreeSet::new();
        let mut reported_depth = false;
        let mut walk = Walk::default();

        for (start, max_depth) in starts {
            if color[start] != WHITE {
                continue;
            }
            color[start] = GRAY;
            walk.reached.insert(start);
            let mut stack: Vec<(usize, usize)> = vec![(start, 0)];

            while let Some(&(node, edge_index)) = stack.last() {
                let Some(edge) = self.edges[node].get(edge_index) else {
                    color[node] = BLACK;
                    stack.pop();
                    continue;
                };
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
                match color[edge.target] {
                    // `stack.len() - 1` is the depth of `node`, so the target
                    // sits one deeper.
                    WHITE if max_depth.is_some_and(|max| stack.len() > max) => {
                        if !reported_depth {
                            reported_depth = true;
                            walk.findings.push(Finding::TooDeep {
                                from: node,
                                edge: edge.clone(),
                            });
                        }
                        color[edge.target] = BLACK;
                    }
                    WHITE => {
                        color[edge.target] = GRAY;
                        walk.reached.insert(edge.target);
                        stack.push((edge.target, 0));
                    }
                    GRAY => {
                        // Back edge: the cycle is the current path from the
                        // target onwards, closed by this edge.
                        let start_of_cycle = stack
                            .iter()
                            .position(|(on_path, _)| *on_path == edge.target)
                            .unwrap_or(0);
                        let nodes: Vec<usize> = stack[start_of_cycle..]
                            .iter()
                            .map(|(on_path, _)| *on_path)
                            .collect();
                        let mut key = nodes.clone();
                        key.sort_unstable();
                        if reported.insert(key) {
                            walk.findings.push(Finding::Cycle(Cycle {
                                nodes,
                                from: node,
                                edge: edge.clone(),
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        walk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::catalog::Catalog;
    use serde_json::json;

    /// Builds a graph of `Card`s, each naming the next by id.
    fn graph(links: &[(&str, &str)]) -> (Graph, Vec<String>) {
        let catalog = Catalog::basic();
        let components: Vec<Component> = links
            .iter()
            .map(|(id, child)| Component::new(*id, "Card").with("child", json!(child)))
            .collect();
        let ids: BTreeMap<&str, usize> = components
            .iter()
            .enumerate()
            .map(|(index, component)| (component.id.as_str(), index))
            .collect();
        let mut dangling = Vec::new();
        let graph = Graph::build(
            |_, component| catalog.references(component),
            components.iter().map(Some),
            &ids,
            |_, _, reference| dangling.push(reference.id),
        );
        (graph, dangling)
    }

    fn cycles(walk: &Walk) -> Vec<&Cycle> {
        walk.findings
            .iter()
            .filter_map(|finding| match finding {
                Finding::Cycle(cycle) => Some(cycle),
                Finding::TooDeep { .. } => None,
            })
            .collect()
    }

    #[test]
    fn references_to_listed_ids_become_edges_and_the_rest_dangle() {
        let (graph, dangling) = graph(&[("root", "a"), ("a", "missing")]);
        assert_eq!(graph.len(), 2);
        assert_eq!(graph.edges(0)[0].target, 1);
        assert_eq!(graph.edges(0)[0].location, "child");
        assert!(graph.edges(1).is_empty());
        assert_eq!(dangling, vec!["missing".to_string()]);
    }

    #[test]
    fn a_walk_from_the_root_reaches_exactly_what_the_root_can() {
        let (graph, _) = graph(&[("root", "a"), ("a", "leaf"), ("orphan", "a"), ("leaf", "-")]);
        let walk = graph.walk([0], None);
        assert_eq!(walk.reached, BTreeSet::from([0, 1, 3]));
        assert!(walk.findings.is_empty());
    }

    #[test]
    fn each_distinct_cycle_is_found_once_whatever_starts_lead_into_it() {
        let (graph, _) = graph(&[("root", "a"), ("a", "b"), ("b", "a")]);
        let walk = graph.walk(0..graph.len(), None);
        let cycles = cycles(&walk);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].nodes, vec![1, 2]);
        assert_eq!(cycles[0].from, 2);
        assert_eq!(cycles[0].edge.target, 1);
    }

    #[test]
    fn a_self_reference_is_the_one_node_cycle() {
        let (graph, _) = graph(&[("root", "a"), ("a", "a")]);
        let walk = graph.walk([0], None);
        let cycles = cycles(&walk);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].nodes, vec![1]);
        assert_eq!(cycles[0].from, 1);
        assert_eq!(cycles[0].edge.target, 1);
    }

    #[test]
    fn a_cycle_the_root_cannot_reach_is_not_found_from_the_root() {
        let (graph, _) = graph(&[("root", "leaf"), ("leaf", "-"), ("x", "y"), ("y", "x")]);
        assert!(graph.walk([0], None).findings.is_empty());
        assert_eq!(cycles(&graph.walk(0..graph.len(), None)).len(), 1);
    }

    #[test]
    fn a_depth_limit_condemns_the_subtree_and_reports_once() {
        // root -> n1 -> n2 -> n3 -> n4, a chain five nodes long.
        let (graph, _) = graph(&[
            ("root", "n1"),
            ("n1", "n2"),
            ("n2", "n3"),
            ("n3", "n4"),
            ("n4", "-"),
        ]);
        // The root is depth 0, so a limit of 2 admits n1 and n2 and stops at
        // the reference from n2 to n3.
        let walk = graph.walk([0], Some(2));
        assert_eq!(walk.reached, BTreeSet::from([0, 1, 2]));
        assert_eq!(walk.findings.len(), 1);
        assert!(matches!(
            &walk.findings[0],
            Finding::TooDeep { from: 2, edge } if edge.target == 3
        ));

        // Without a limit the same graph is walked to the end.
        assert_eq!(graph.walk([0], None).reached.len(), 5);
    }

    #[test]
    fn a_very_deep_chain_is_walked_without_recursing() {
        let depth = 50_000;
        let names: Vec<String> = (0..=depth).map(|i| format!("n{i}")).collect();
        let links: Vec<(&str, &str)> = (0..depth)
            .map(|i| (names[i].as_str(), names[i + 1].as_str()))
            .collect();
        let (graph, dangling) = graph(&links);
        assert_eq!(dangling.len(), 1, "only the last link dangles");
        assert_eq!(graph.walk([0], None).reached.len(), depth);
    }
}
