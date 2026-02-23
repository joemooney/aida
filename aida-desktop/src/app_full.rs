use eframe::egui;
use aida_core::{
    Requirement, RequirementPriority, RequirementStatus, RequirementType,
    RequirementsStore, Storage, RelationshipType, determine_requirements_path,
};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Default, PartialEq, Clone)]
enum View {
    #[default]
    List,
    Detail(Uuid),
    Add,
    Edit(Uuid),
}

pub struct RequirementsApp {
    storage: Storage,
    store: RequirementsStore,
    current_view: View,
    selected_requirement: Option<Uuid>,

    // Filter state
    filter_text: String,
    filter_status: Option<RequirementStatus>,
    filter_priority: Option<RequirementPriority>,
    filter_feature: Option<String>,

    // Add/Edit form state
    form_title: String,
    form_description: String,
    form_status: RequirementStatus,
    form_priority: RequirementPriority,
    form_type: RequirementType,
    form_owner: String,
    form_feature: String,
    form_tags: String,

    // Relationship form state
    show_add_relationship: bool,
    rel_target_id: String,
    rel_type: String,
    rel_bidirectional: bool,

    // Error/success messages
    message: Option<(String, bool)>, // (message, is_error)
}

impl RequirementsApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // Load requirements
        let requirements_path = determine_requirements_path(None)
            .unwrap_or_else(|_| std::path::PathBuf::from("requirements.yaml"));

        let storage = Storage::new(requirements_path);
        let store = storage.load().unwrap_or_else(|_| RequirementsStore::new());

        Self {
            storage,
            store,
            current_view: View::List,
            selected_requirement: None,
            filter_text: String::new(),
            filter_status: None,
            filter_priority: None,
            filter_feature: None,
            form_title: String::new(),
            form_description: String::new(),
            form_status: RequirementStatus::Draft,
            form_priority: RequirementPriority::Medium,
            form_type: RequirementType::Functional,
            form_owner: String::new(),
            form_feature: String::from("Uncategorized"),
            form_tags: String::new(),
            show_add_relationship: false,
            rel_target_id: String::new(),
            rel_type: String::from("references"),
            rel_bidirectional: false,
            message: None,
        }
    }

    fn reload(&mut self) {
        if let Ok(store) = self.storage.load() {
            self.store = store;
        }
    }

    fn save(&mut self) {
        if let Err(e) = self.storage.save(&self.store) {
            self.message = Some((format!("Error saving: {}", e), true));
        } else {
            self.message = Some(("Saved successfully".to_string(), false));
        }
    }

    fn clear_form(&mut self) {
        self.form_title.clear();
        self.form_description.clear();
        self.form_status = RequirementStatus::Draft;
        self.form_priority = RequirementPriority::Medium;
        self.form_type = RequirementType::Functional;
        self.form_owner.clear();
        self.form_feature = String::from("Uncategorized");
        self.form_tags.clear();
    }

    fn load_requirement_into_form(&mut self, id: &Uuid) {
        if let Some(req) = self.store.get_requirement_by_id(id) {
            self.form_title = req.title.clone();
            self.form_description = req.description.clone();
            self.form_status = req.status.clone();
            self.form_priority = req.priority.clone();
            self.form_type = req.req_type.clone();
            self.form_owner = req.owner.clone();
            self.form_feature = req.feature.clone();
            self.form_tags = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        }
    }
}

impl eframe::App for RequirementsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top menu bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Reload").clicked() {
                        self.reload();
                        ui.close_menu();
                    }
                    if ui.button("Save").clicked() {
                        self.save();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("View", |ui| {
                    if ui.button("Requirements List").clicked() {
                        self.current_view = View::List;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Add", |ui| {
                    if ui.button("New Requirement").clicked() {
                        self.clear_form();
                        self.current_view = View::Add;
                        ui.close_menu();
                    }
                });
            });
        });

        // Show message bar if there's a message
        let has_message = self.message.is_some();
        if has_message {
            let (msg, is_error) = self.message.as_ref().unwrap().clone();
            egui::TopBottomPanel::top("message_panel").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let color = if is_error {
                        egui::Color32::from_rgb(200, 50, 50)
                    } else {
                        egui::Color32::from_rgb(50, 150, 50)
                    };

                    ui.colored_label(color, &msg);

                    if ui.button("✖").clicked() {
                        self.message = None;
                    }
                });
            });
        }

        // Main content area
        let current_view = self.current_view.clone();
        egui::CentralPanel::default().show(ctx, |ui| {
            match current_view {
                View::List => self.render_list_view(ui),
                View::Detail(id) => self.render_detail_view(ui, &id),
                View::Add => self.render_add_view(ui),
                View::Edit(id) => self.render_edit_view(ui, &id),
            }
        });
    }
}

// Implement views (separate methods for clarity)
impl RequirementsApp {
    fn render_list_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Requirements");

        ui.separator();

        // Filter controls
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.filter_text);

            ui.separator();

            ui.label("Status:");
            egui::ComboBox::from_id_salt("status_filter")
                .selected_text(if let Some(s) = &self.filter_status {
                    format!("{:?}", s)
                } else {
                    "All".to_string()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter_status, None, "All");
                    ui.selectable_value(&mut self.filter_status, Some(RequirementStatus::Draft), "Draft");
                    ui.selectable_value(&mut self.filter_status, Some(RequirementStatus::Approved), "Approved");
                    ui.selectable_value(&mut self.filter_status, Some(RequirementStatus::Completed), "Completed");
                    ui.selectable_value(&mut self.filter_status, Some(RequirementStatus::Rejected), "Rejected");
                });

            ui.separator();

            ui.label("Priority:");
            egui::ComboBox::from_id_salt("priority_filter")
                .selected_text(if let Some(p) = &self.filter_priority {
                    format!("{:?}", p)
                } else {
                    "All".to_string()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter_priority, None, "All");
                    ui.selectable_value(&mut self.filter_priority, Some(RequirementPriority::High), "High");
                    ui.selectable_value(&mut self.filter_priority, Some(RequirementPriority::Medium), "Medium");
                    ui.selectable_value(&mut self.filter_priority, Some(RequirementPriority::Low), "Low");
                });
        });

        ui.separator();

        // Requirements table
        use egui_extras::{TableBuilder, Column};

        let text_height = egui::TextStyle::Body.resolve(ui.style()).size;

        // Track clicked action
        let mut clicked_view: Option<Uuid> = None;
        let mut clicked_edit: Option<Uuid> = None;

        let filtered_requirements: Vec<&Requirement> = self.store.requirements
            .iter()
            .filter(|req| {
                // Apply filters
                if !self.filter_text.is_empty() {
                    let search = self.filter_text.to_lowercase();
                    if !req.title.to_lowercase().contains(&search)
                        && !req.description.to_lowercase().contains(&search) {
                        return false;
                    }
                }

                if let Some(status) = &self.filter_status {
                    if &req.status != status {
                        return false;
                    }
                }

                if let Some(priority) = &self.filter_priority {
                    if &req.priority != priority {
                        return false;
                    }
                }

                true
            })
            .collect();

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto()) // SPEC-ID
            .column(Column::remainder().at_least(200.0)) // Title
            .column(Column::auto()) // Status
            .column(Column::auto()) // Priority
            .column(Column::auto()) // Type
            .column(Column::auto().at_least(100.0)) // Feature
            .column(Column::auto()) // Actions
            .header(20.0, |mut header| {
                header.col(|ui| { ui.strong("SPEC-ID"); });
                header.col(|ui| { ui.strong("Title"); });
                header.col(|ui| { ui.strong("Status"); });
                header.col(|ui| { ui.strong("Priority"); });
                header.col(|ui| { ui.strong("Type"); });
                header.col(|ui| { ui.strong("Feature"); });
                header.col(|ui| { ui.strong("Actions"); });
            })
            .body(|mut body| {
                for req in filtered_requirements {
                    body.row(text_height * 1.5, |mut row| {
                        let req_id = req.id;

                        row.col(|ui| {
                            if let Some(spec_id) = &req.spec_id {
                                ui.label(spec_id);
                            } else {
                                ui.label("-");
                            }
                        });

                        row.col(|ui| {
                            ui.label(&req.title);
                        });

                        row.col(|ui| {
                            let color = match req.status {
                                RequirementStatus::Draft => egui::Color32::from_rgb(200, 200, 50),
                                RequirementStatus::Approved => egui::Color32::from_rgb(50, 150, 200),
                                RequirementStatus::Completed => egui::Color32::from_rgb(50, 200, 50),
                                RequirementStatus::Rejected => egui::Color32::from_rgb(200, 50, 50),
                            };
                            ui.colored_label(color, format!("{:?}", req.status));
                        });

                        row.col(|ui| {
                            let color = match req.priority {
                                RequirementPriority::High => egui::Color32::from_rgb(200, 50, 50),
                                RequirementPriority::Medium => egui::Color32::from_rgb(200, 150, 50),
                                RequirementPriority::Low => egui::Color32::from_rgb(50, 200, 50),
                            };
                            ui.colored_label(color, format!("{:?}", req.priority));
                        });

                        row.col(|ui| {
                            ui.label(format!("{:?}", req.req_type));
                        });

                        row.col(|ui| {
                            ui.label(&req.feature);
                        });

                        row.col(|ui| {
                            if ui.button("View").clicked() {
                                clicked_view = Some(req_id);
                            }
                            if ui.button("Edit").clicked() {
                                clicked_edit = Some(req_id);
                            }
                        });
                    });
                }
            });

        let filtered_count = filtered_requirements.len();

        ui.separator();
        ui.label(format!("Showing {} of {} requirements", filtered_count, self.store.requirements.len()));

        // Handle button clicks outside the table closure
        if let Some(id) = clicked_view {
            self.current_view = View::Detail(id);
        }
        if let Some(id) = clicked_edit {
            self.load_requirement_into_form(&id);
            self.current_view = View::Edit(id);
        }
    }

    fn render_detail_view(&mut self, ui: &mut egui::Ui, id: &Uuid) {
        if ui.button("← Back to List").clicked() {
            self.current_view = View::List;
            return;
        }

        ui.separator();

        let req = match self.store.get_requirement_by_id(id) {
            Some(r) => r,
            None => {
                ui.label("Requirement not found");
                return;
            }
        };

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading(&req.title);

            ui.separator();

            egui::Grid::new("detail_grid")
                .num_columns(2)
                .spacing([40.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label("UUID:");
                    ui.label(req.id.to_string());
                    ui.end_row();

                    if let Some(spec_id) = &req.spec_id {
                        ui.label("SPEC-ID:");
                        ui.label(spec_id);
                        ui.end_row();
                    }

                    ui.label("Status:");
                    ui.label(format!("{:?}", req.status));
                    ui.end_row();

                    ui.label("Priority:");
                    ui.label(format!("{:?}", req.priority));
                    ui.end_row();

                    ui.label("Type:");
                    ui.label(format!("{:?}", req.req_type));
                    ui.end_row();

                    ui.label("Owner:");
                    ui.label(&req.owner);
                    ui.end_row();

                    ui.label("Feature:");
                    ui.label(&req.feature);
                    ui.end_row();

                    ui.label("Created:");
                    ui.label(req.created_at.format("%Y-%m-%d %H:%M:%S").to_string());
                    ui.end_row();

                    ui.label("Modified:");
                    ui.label(req.modified_at.format("%Y-%m-%d %H:%M:%S").to_string());
                    ui.end_row();
                });

            ui.separator();

            ui.heading("Description");
            ui.label(&req.description);

            if !req.tags.is_empty() {
                ui.separator();
                ui.heading("Tags");
                ui.horizontal_wrapped(|ui| {
                    for tag in &req.tags {
                        ui.label(egui::RichText::new(tag).background_color(egui::Color32::from_rgb(100, 100, 150)));
                    }
                });
            }

            if !req.relationships.is_empty() {
                ui.separator();
                ui.heading("Relationships");

                for rel in &req.relationships {
                    if let Some(target) = self.store.get_requirement_by_id(&rel.target_id) {
                        let target_spec = target.spec_id.as_deref().unwrap_or("N/A");
                        let description = match &rel.rel_type {
                            RelationshipType::Parent => "is parent of",
                            RelationshipType::Child => "is child of",
                            RelationshipType::Duplicate => "is duplicate of",
                            RelationshipType::Verifies => "verifies",
                            RelationshipType::VerifiedBy => "is verified by",
                            RelationshipType::References => "references",
                            RelationshipType::Custom(name) => name,
                        };

                        ui.horizontal(|ui| {
                            ui.label(format!("• {} {} - {}", description, target_spec, target.title));
                            if ui.small_button("View").clicked() {
                                self.current_view = View::Detail(target.id);
                            }
                        });
                    }
                }
            }

            ui.separator();

            ui.horizontal(|ui| {
                if ui.button("Edit").clicked() {
                    self.load_requirement_into_form(id);
                    self.current_view = View::Edit(*id);
                }

                if ui.button("Add Relationship").clicked() {
                    self.show_add_relationship = true;
                }

                if ui.button("Delete").clicked() {
                    // TODO: Add confirmation dialog
                    self.store.requirements.retain(|r| r.id != *id);
                    self.save();
                    self.current_view = View::List;
                }
            });

            // Relationship dialog
            if self.show_add_relationship {
                egui::Window::new("Add Relationship")
                    .collapsible(false)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        ui.label("Target SPEC-ID or UUID:");
                        ui.text_edit_singleline(&mut self.rel_target_id);

                        ui.label("Relationship Type:");
                        egui::ComboBox::from_label("")
                            .selected_text(&self.rel_type)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.rel_type, "parent".to_string(), "parent");
                                ui.selectable_value(&mut self.rel_type, "child".to_string(), "child");
                                ui.selectable_value(&mut self.rel_type, "duplicate".to_string(), "duplicate");
                                ui.selectable_value(&mut self.rel_type, "verifies".to_string(), "verifies");
                                ui.selectable_value(&mut self.rel_type, "verified-by".to_string(), "verified-by");
                                ui.selectable_value(&mut self.rel_type, "references".to_string(), "references");
                            });

                        ui.checkbox(&mut self.rel_bidirectional, "Bidirectional");

                        ui.horizontal(|ui| {
                            if ui.button("Add").clicked() {
                                // Parse target ID
                                let target_uuid = if let Ok(uuid) = Uuid::parse_str(&self.rel_target_id) {
                                    Some(uuid)
                                } else {
                                    self.store.get_requirement_by_spec_id(&self.rel_target_id)
                                        .map(|r| r.id)
                                };

                                if let Some(target_id) = target_uuid {
                                    let rel_type = RelationshipType::from_str(&self.rel_type);
                                    if let Err(e) = self.store.add_relationship(
                                        id,
                                        rel_type,
                                        &target_id,
                                        self.rel_bidirectional
                                    ) {
                                        self.message = Some((format!("Error: {}", e), true));
                                    } else {
                                        self.save();
                                        self.show_add_relationship = false;
                                        self.rel_target_id.clear();
                                    }
                                } else {
                                    self.message = Some(("Invalid target ID".to_string(), true));
                                }
                            }

                            if ui.button("Cancel").clicked() {
                                self.show_add_relationship = false;
                                self.rel_target_id.clear();
                            }
                        });
                    });
            }
        });
    }

    fn render_add_view(&mut self, ui: &mut egui::Ui) {
        if ui.button("← Back to List").clicked() {
            self.current_view = View::List;
            return;
        }

        ui.separator();
        ui.heading("Add New Requirement");
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            self.render_requirement_form(ui);

            ui.separator();

            if ui.button("Create Requirement").clicked() {
                let mut req = Requirement::new(self.form_title.clone(), self.form_description.clone());
                req.status = self.form_status.clone();
                req.priority = self.form_priority.clone();
                req.req_type = self.form_type.clone();
                req.owner = self.form_owner.clone();
                req.feature = self.form_feature.clone();

                // Parse tags
                if !self.form_tags.is_empty() {
                    req.tags = self.form_tags.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }

                self.store.add_requirement_with_spec_id(req);
                self.save();
                self.clear_form();
                self.current_view = View::List;
            }
        });
    }

    fn render_edit_view(&mut self, ui: &mut egui::Ui, id: &Uuid) {
        if ui.button("← Back to Detail").clicked() {
            self.current_view = View::Detail(*id);
            return;
        }

        ui.separator();
        ui.heading("Edit Requirement");
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            self.render_requirement_form(ui);

            ui.separator();

            if ui.button("Save Changes").clicked() {
                if let Some(req) = self.store.get_requirement_by_id_mut(id) {
                    req.title = self.form_title.clone();
                    req.description = self.form_description.clone();
                    req.status = self.form_status.clone();
                    req.priority = self.form_priority.clone();
                    req.req_type = self.form_type.clone();
                    req.owner = self.form_owner.clone();
                    req.feature = self.form_feature.clone();

                    // Parse tags
                    req.tags = if !self.form_tags.is_empty() {
                        self.form_tags.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    } else {
                        HashSet::new()
                    };

                    req.modified_at = chrono::Utc::now();
                }

                self.save();
                self.current_view = View::Detail(*id);
            }
        });
    }

    fn render_requirement_form(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("requirement_form")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Title:");
                ui.text_edit_singleline(&mut self.form_title);
                ui.end_row();

                ui.label("Status:");
                egui::ComboBox::from_id_salt("form_status")
                    .selected_text(format!("{:?}", self.form_status))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.form_status, RequirementStatus::Draft, "Draft");
                        ui.selectable_value(&mut self.form_status, RequirementStatus::Approved, "Approved");
                        ui.selectable_value(&mut self.form_status, RequirementStatus::Completed, "Completed");
                        ui.selectable_value(&mut self.form_status, RequirementStatus::Rejected, "Rejected");
                    });
                ui.end_row();

                ui.label("Priority:");
                egui::ComboBox::from_id_salt("form_priority")
                    .selected_text(format!("{:?}", self.form_priority))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.form_priority, RequirementPriority::High, "High");
                        ui.selectable_value(&mut self.form_priority, RequirementPriority::Medium, "Medium");
                        ui.selectable_value(&mut self.form_priority, RequirementPriority::Low, "Low");
                    });
                ui.end_row();

                ui.label("Type:");
                egui::ComboBox::from_id_salt("form_type")
                    .selected_text(format!("{:?}", self.form_type))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.form_type, RequirementType::Functional, "Functional");
                        ui.selectable_value(&mut self.form_type, RequirementType::NonFunctional, "Non-Functional");
                        ui.selectable_value(&mut self.form_type, RequirementType::System, "System");
                        ui.selectable_value(&mut self.form_type, RequirementType::User, "User");
                    });
                ui.end_row();

                ui.label("Owner:");
                ui.text_edit_singleline(&mut self.form_owner);
                ui.end_row();

                ui.label("Feature:");
                ui.text_edit_singleline(&mut self.form_feature);
                ui.end_row();

                ui.label("Tags (comma-separated):");
                ui.text_edit_singleline(&mut self.form_tags);
                ui.end_row();
            });

        ui.separator();

        ui.label("Description:");
        ui.text_edit_multiline(&mut self.form_description);
    }
}
