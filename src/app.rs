use crate::graph::Graph;
use crate::package_manager::{PackageSource, get_all_packages};
use eframe::egui;
use egui::{Color32, Pos2, Stroke, Vec2};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Runtime;

enum PackageLoadState {
    Loading,
    Ready(Graph),
    Failed(String),
}

pub struct LinuxGraphApp {
    package_state: Arc<Mutex<PackageLoadState>>,
    pan: Vec2,
    zoom: f32,
    selected_node: Option<usize>,
    search_query: String,
    _rt: Runtime,
}

impl LinuxGraphApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark()); // Темная тема по умолчанию

        let rt = Runtime::new().unwrap();
        let package_state = Arc::new(Mutex::new(PackageLoadState::Loading));

        let state_clone = package_state.clone();
        rt.spawn(async move {
            let next_state = match get_all_packages().await {
                Ok(packages) => PackageLoadState::Ready(Graph::new(packages)),
                Err(error) => PackageLoadState::Failed(error.to_string()),
            };

            *state_clone.lock().unwrap() = next_state;
        });

        Self {
            package_state,
            pan: Vec2::ZERO,
            zoom: 1.0,
            selected_node: None,
            search_query: String::new(),
            _rt: rt,
        }
    }
}

impl eframe::App for LinuxGraphApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let loading = {
            let state = self.package_state.lock().unwrap();
            matches!(&*state, PackageLoadState::Loading)
        };
        if loading {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let mut state_lock = self.package_state.lock().unwrap();
        let search_query_lower = self.search_query.trim().to_lowercase();

        let mut frame = egui::Frame::side_top_panel(&ctx.global_style());
        frame.inner_margin = egui::Margin::same(15); // Красивые отступы

        egui::Panel::left("sidebar")
            .exact_size(460.0)
            .frame(frame)
            .show_inside(ui, |ui| {
                // Кнопка центрирования в самом низу панели
                egui::Panel::bottom("sidebar_bottom").show_inside(ui, |ui| {
                    ui.add_space(10.0);
                    if ui
                        .add_sized(
                            [ui.available_width(), 40.0],
                            egui::Button::new(egui::RichText::new("🎯 Center Graph").size(16.0)),
                        )
                        .clicked()
                    {
                        self.pan = egui::Vec2::ZERO;
                        self.zoom = 0.5;
                        self.selected_node = None;
                    }
                    ui.add_space(10.0);
                });

                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    ui.heading(
                        egui::RichText::new("Linux Dependencies")
                            .size(24.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                });
                ui.add_space(10.0);
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Search:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .desired_width(f32::INFINITY),
                    );
                });

                ui.add_space(10.0);
                ui.separator();

                // Автодополнение (Programmable Completion)
                if !search_query_lower.is_empty()
                    && let PackageLoadState::Ready(graph) = &*state_lock
                {
                    let exact_match = graph
                        .nodes
                        .iter()
                        .any(|n| n.info.name.eq_ignore_ascii_case(&self.search_query));

                    if !exact_match {
                        let matches: Vec<_> = graph
                            .nodes
                            .iter()
                            .enumerate()
                            .filter(|(_, n)| {
                                n.info.name.to_lowercase().starts_with(&search_query_lower)
                            })
                            .take(6)
                            .map(|(idx, n)| (idx, n.info.name.clone()))
                            .collect();

                        if !matches.is_empty() {
                            ui.group(|ui| {
                                ui.label(
                                    egui::RichText::new("Suggestions:")
                                        .color(Color32::from_gray(150)),
                                );
                                for (idx, name) in matches {
                                    if ui.selectable_label(false, &name).clicked() {
                                        self.search_query = name;

                                        // Автоматически фокусируемся на пакете при выборе
                                        self.selected_node = Some(idx);
                                        self.pan = -graph.nodes[idx].pos.to_vec2() * self.zoom;
                                    }
                                }
                            });
                        }
                    }
                }

                ui.separator();

                match &mut *state_lock {
                    PackageLoadState::Ready(graph) => {
                        ui.heading(
                            egui::RichText::new("🌌 Linux Package Graph")
                                .size(20.0)
                                .strong()
                                .color(egui::Color32::from_rgb(150, 200, 255)),
                        );
                        ui.add_space(5.0);

                        // Считаем статистику
                        let mut native_count = 0;
                        let mut foreign_count = 0;
                        let mut flatpak_count = 0;
                        let mut total_size_kb = 0.0;

                        for node in &graph.nodes {
                            total_size_kb += node.info.size_kb;
                            match node.info.source {
                                PackageSource::Native => native_count += 1,
                                PackageSource::Foreign => foreign_count += 1,
                                PackageSource::Flatpak => flatpak_count += 1,
                            }
                        }

                        ui.label(
                            egui::RichText::new(format!(
                                "Total packages: {}",
                                native_count + foreign_count + flatpak_count
                            ))
                            .strong(),
                        );
                        ui.label(format!(
                            "Total size: {:.2} GB",
                            total_size_kb / 1024.0 / 1024.0
                        ));

                        ui.add_space(10.0);
                        ui.separator();

                        // Легенда
                        ui.label(egui::RichText::new("Legend:").strong());
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("⬤")
                                    .color(egui::Color32::from_rgb(100, 150, 250)),
                            );
                            ui.label(format!("System ({})", native_count));
                        });
                        if foreign_count > 0 {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("⬤")
                                        .color(egui::Color32::from_rgb(250, 100, 100)),
                                );
                                ui.label(format!("AUR/Foreign ({})", foreign_count));
                            });
                        }
                        if flatpak_count > 0 {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("⬤")
                                        .color(egui::Color32::from_rgb(50, 200, 100)),
                                );
                                ui.label(format!("Flatpak ({})", flatpak_count));
                            });
                        }

                        ui.add_space(10.0);
                        ui.separator();

                        if let Some(idx) = self.selected_node.filter(|&idx| idx < graph.nodes.len())
                        {
                            let node = &graph.nodes[idx];
                            ui.label(egui::RichText::new("Package Details").size(16.0).strong());
                            ui.add_space(5.0);

                            ui.label(
                                egui::RichText::new(format!("Name: {}", node.info.name))
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            );
                            ui.label(format!("Version: {}", node.info.version));
                            ui.label(format!("Size: {:.2} MB", node.info.size_kb / 1024.0));
                            ui.separator();
                            ui.label("Description:");
                            ui.label(&node.info.description);
                            ui.separator();

                            let mut jump_to = None;

                            ui.label(format!("Depends On ({}):", node.info.depends_on.len()));
                            egui::ScrollArea::vertical()
                                .id_salt("deps")
                                .max_height(150.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    for dep in &node.info.depends_on {
                                        if ui.link(dep).clicked()
                                            && let Some(&i) = graph.name_to_index.get(dep)
                                        {
                                            jump_to = Some(i);
                                        }
                                    }
                                });
                            ui.separator();
                            ui.label(format!("Required By ({}):", node.info.required_by.len()));
                            egui::ScrollArea::vertical()
                                .id_salt("reqs")
                                .max_height(150.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    for req in &node.info.required_by {
                                        if ui.link(req).clicked()
                                            && let Some(&i) = graph.name_to_index.get(req)
                                        {
                                            jump_to = Some(i);
                                        }
                                    }
                                });

                            if let Some(j) = jump_to {
                                self.selected_node = Some(j);
                                // Плавно переносим камеру на выбранный узел
                                self.pan = -graph.nodes[j].pos.to_vec2() * self.zoom;
                            }
                        } else {
                            self.selected_node = None;
                            ui.label("Select a package to view details.");
                        }
                    }
                    PackageLoadState::Loading => {
                        ui.spinner();
                        ui.label("Loading packages...");
                    }
                    PackageLoadState::Failed(error) => {
                        ui.label(egui::RichText::new("Failed to load packages").strong());
                        ui.add_space(5.0);
                        ui.label(error.as_str());
                    }
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            let (response, painter) =
                ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());

            let rect = response.rect;
            let screen_rect = rect;

            // Handle Zoom & Pan
            if response.hovered() {
                let zoom_delta = ctx.input(|i| i.smooth_scroll_delta.y);
                if zoom_delta != 0.0 {
                    let old_zoom = self.zoom;
                    // Используем экспоненциальное масштабирование для абсолютно плавного зума (scroll up и down симметричны)
                    self.zoom = (self.zoom * (zoom_delta * 0.002_f32).exp()).clamp(0.03, 10.0);

                    if let Some(pointer_pos) = ctx.pointer_hover_pos() {
                        let center = rect.center().to_vec2();
                        // Определяем точку мира под курсором до зума
                        let target_w = (pointer_pos.to_vec2() - center - self.pan) / old_zoom;
                        // Двигаем камеру так, чтобы эта точка мира осталась ровно под курсором
                        self.pan += target_w * (old_zoom - self.zoom);
                    }
                }
            }

            if response.dragged() {
                self.pan += response.drag_delta();
            }

            let center = rect.center() + self.pan;

            if let PackageLoadState::Ready(graph) = &mut *state_lock {
                // Физика отключена в реальном времени!
                // graph.apply_forces(0.5);

                // Transform function: world pos to screen pos
                let to_screen = |p: Pos2| -> Pos2 { center + (p.to_vec2() - self.pan) * self.zoom };

                // Отрисовка космических орбит (Эстетика Солнечной системы)
                let screen_center = to_screen(Pos2::new(0.0, 0.0));
                for i in 1..=30 {
                    let r = (i as f32) * 200.0; // Орбиты каждые 200 мировых пикселей
                    let screen_radius = r * self.zoom;
                    // Отрисовываем только если орбита имеет осмысленный размер на экране
                    if screen_radius > 5.0 && screen_radius < 5000.0 {
                        painter.circle_stroke(
                            screen_center,
                            screen_radius,
                            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(100)), // Сделал орбиты в 3.5 раза ярче!
                        );
                    }
                }

                // Предрасчет позиций на экране для всех узлов
                let mut screen_positions = Vec::with_capacity(graph.nodes.len());
                for node in &graph.nodes {
                    screen_positions.push(to_screen(node.pos));
                }

                // 1. Oпределение Hover
                let pointer_pos = ctx.pointer_hover_pos();
                let mut hovered_node = None;

                if let Some(pp) = pointer_pos {
                    for (i, node) in graph.nodes.iter().enumerate() {
                        let screen_pos = screen_positions[i];
                        let screen_radius = node.radius * self.zoom;
                        if rect.expand(screen_radius).contains(screen_pos)
                            && screen_pos.distance(pp) < screen_radius.max(5.0)
                        {
                            hovered_node = Some(i);
                            break;
                        }
                    }
                }

                // 2. Отрисовка ребер (еле заметная паутина)
                for edge in &graph.edges {
                    let p1 = screen_positions[edge.from];
                    let p2 = screen_positions[edge.to];

                    // Грубая отсечка: если обе точки за экраном, не рисуем
                    if !screen_rect.expand(100.0).contains(p1)
                        && !screen_rect.expand(100.0).contains(p2)
                    {
                        continue;
                    }

                    // Линии всегда одинаковые: очень тонкие и еле-еле заметные (alpha=8)
                    painter.line_segment([p1, p2], Stroke::new(0.5, Color32::from_white_alpha(8)));
                }

                // 3. Отрисовываем Узлы
                for (i, node) in graph.nodes.iter_mut().enumerate() {
                    let screen_pos = screen_positions[i];
                    let screen_radius = node.radius * self.zoom;

                    if !rect.expand(screen_radius).contains(screen_pos) {
                        continue;
                    }

                    let is_selected = self.selected_node == Some(i);
                    let is_hovered = hovered_node == Some(i);

                    let search_match = !search_query_lower.is_empty()
                        && (node.info.name.to_lowercase().contains(&search_query_lower)
                            || node
                                .info
                                .description
                                .to_lowercase()
                                .contains(&search_query_lower));

                    let (fill_color, stroke) = if is_selected {
                        (
                            Color32::from_rgb(100, 200, 255),
                            Stroke::new(2.0, Color32::WHITE),
                        )
                    } else if search_match {
                        (
                            Color32::from_rgb(255, 200, 50),
                            Stroke::new(1.5, Color32::WHITE),
                        )
                    } else if is_hovered {
                        (
                            Color32::from_rgb(150, 250, 150),
                            Stroke::new(1.5, Color32::WHITE),
                        )
                    } else if (self.selected_node.is_some() && !is_selected)
                        || (!self.search_query.is_empty())
                    {
                        (
                            Color32::from_rgba_unmultiplied(100, 100, 100, 20),
                            Stroke::NONE,
                        )
                    } else {
                        let color = match node.info.source {
                            PackageSource::Native => Color32::from_rgb(100, 150, 250), // Синий (системные)
                            PackageSource::Foreign => Color32::from_rgb(250, 100, 100), // Красный (AUR/сторонние)
                            PackageSource::Flatpak => Color32::from_rgb(50, 200, 100), // Зеленый (Flatpak)
                        };
                        (color, Stroke::NONE)
                    };

                    let draw_radius = if search_match {
                        screen_radius * 1.5
                    } else {
                        screen_radius
                    };
                    painter.circle(screen_pos, draw_radius, fill_color, stroke);

                    // Отрисовываем имена ВСЕГДА, даже при максимальном отдалении
                    let font_size = 14.0 * self.zoom;
                    let clamped_font_size = font_size.clamp(8.0, 32.0);

                    let text_color = if search_match || is_selected || is_hovered {
                        Color32::WHITE
                    } else if (self.selected_node.is_some() && !is_selected)
                        || (!self.search_query.is_empty())
                    {
                        Color32::from_white_alpha(15) // Сильно приглушаем неактивные узлы и несовпадающие в поиске
                    } else {
                        let alpha = (150.0 * self.zoom.clamp(0.2, 1.0)) as u8;
                        Color32::from_white_alpha(alpha.max(30))
                    };

                    painter.text(
                        screen_pos + Vec2::new(0.0, draw_radius + 4.0),
                        egui::Align2::CENTER_TOP,
                        &node.info.name,
                        egui::FontId::proportional(clamped_font_size),
                        text_color,
                    );
                }

                // Handle Click
                if response.clicked() {
                    if let Some(idx) = hovered_node {
                        self.selected_node = Some(idx);
                    } else {
                        self.selected_node = None;
                    }
                }
            } else if let PackageLoadState::Failed(error) = &*state_lock {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("Failed to load packages:\n{error}"),
                    egui::FontId::proportional(16.0),
                    Color32::from_rgb(250, 120, 120),
                );
            }
        });
    }
}
