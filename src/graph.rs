use crate::package_manager::PackageInfo;
use egui::Pos2;
use std::collections::HashMap;

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
    pub _name_to_index: HashMap<String, usize>,
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
        for (i, node) in nodes.iter().enumerate() {
            for dep in &node.info.depends_on {
                if let Some(&j) = name_to_index.get(dep) {
                    edges.push(Edge { from: i, to: j });
                }
            }
        }

        Graph {
            nodes,
            edges,
            _name_to_index: name_to_index,
        }
    }
}
