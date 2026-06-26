use crate::package_manager::PackageInfo;
use egui::Pos2;
use std::collections::{HashMap, HashSet};

pub struct Node {
    pub info: PackageInfo,
    pub pos: Pos2,
    pub radius: f32,
}

pub struct Edge {
    pub from: usize,
    pub to: usize,
}

pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub name_to_index: HashMap<String, usize>,
}

impl Graph {
    pub fn new(packages: HashMap<String, PackageInfo>) -> Self {
        let num_packages = packages.len();
        let mut nodes = Vec::with_capacity(num_packages);
        let mut name_to_index = HashMap::with_capacity(num_packages);

        let mut pkg_vec: Vec<(String, PackageInfo)> = packages.into_iter().collect();
        // Самые важные узлы (с большим числом зависимостей) помещаем в центр спирали (индекс ближе к 0)
        // Пакеты Flatpak и Snap почти не имеют обратных зависимостей, поэтому они автоматически
        // окажутся на самых дальних орбитах, формируя красивое внешнее кольцо песочниц вокруг ядра системы!
        pkg_vec.sort_by_key(|b| std::cmp::Reverse(b.1.required_by.len()));

        for (i, (_, pkg)) in pkg_vec.into_iter().enumerate() {
            let radius = (pkg.depends_on.len() as f32 + pkg.required_by.len() as f32)
                .sqrt()
                .max(2.0);

            // Математически идеальная спираль Фибоначчи
            let golden_angle = 2.399_963_1_f32;
            let theta = i as f32 * golden_angle;
            let r = (i as f32).sqrt() * 200.0;

            name_to_index.insert(pkg.name.clone(), i);

            nodes.push(Node {
                info: pkg,
                pos: Pos2::new(theta.cos() * r, theta.sin() * r),
                radius,
            });
        }

        let mut edges = Vec::new();
        let mut seen_edges = HashSet::new();
        for (i, node) in nodes.iter().enumerate() {
            for dep in &node.info.depends_on {
                if let Some(&j) = name_to_index.get(dep)
                    && i != j
                    && seen_edges.insert((i, j))
                {
                    edges.push(Edge { from: i, to: j });
                }
            }
        }

        Graph {
            nodes,
            edges,
            name_to_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_manager::{PackageInfo, PackageSource};

    fn package(name: &str, depends_on: Vec<&str>, required_by_count: usize) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            size_kb: 0.0,
            depends_on: depends_on.into_iter().map(str::to_string).collect(),
            required_by: (0..required_by_count).map(|i| format!("req-{i}")).collect(),
            source: PackageSource::Native,
        }
    }

    #[test]
    fn graph_deduplicates_edges_and_ignores_self_loops() {
        let mut packages = HashMap::new();
        packages.insert(
            "app".to_string(),
            package("app", vec!["lib", "lib", "app"], 0),
        );
        packages.insert("lib".to_string(), package("lib", Vec::new(), 1));

        let graph = Graph::new(packages);

        assert_eq!(graph.edges.len(), 1);
        let app_idx = graph.name_to_index["app"];
        let lib_idx = graph.name_to_index["lib"];
        assert_eq!(graph.edges[0].from, app_idx);
        assert_eq!(graph.edges[0].to, lib_idx);
    }

    #[test]
    fn graph_sorts_more_required_packages_toward_center() {
        let mut packages = HashMap::new();
        packages.insert("leaf".to_string(), package("leaf", Vec::new(), 0));
        packages.insert("core".to_string(), package("core", Vec::new(), 10));

        let graph = Graph::new(packages);

        assert_eq!(graph.nodes[0].info.name, "core");
        assert_eq!(graph.nodes[0].pos, Pos2::new(0.0, 0.0));
    }
}
