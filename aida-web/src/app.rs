// trace:FR-0273 | ai:claude:high
// trace:AUTH-0001 | ai:claude:high
//! Main AIDA Web application using egui

use std::cell::RefCell;
use std::rc::Rc;

use egui::{Color32, RichText, ScrollArea, TextEdit, Ui};

use crate::client::AidaClient;
// Use generated proto types for all gRPC operations
use crate::generated::aida::*;

// Type conversion for aida_gui UI component compatibility
// Both proto types are generated from the same .proto file but are distinct Rust types
mod gui_convert {
    use super::*;

    /// Convert generated Timestamp to aida_gui Timestamp
    fn to_gui_timestamp(ts: &Option<Timestamp>) -> Option<aida_gui::storage::proto::Timestamp> {
        ts.as_ref().map(|t| aida_gui::storage::proto::Timestamp {
            seconds: t.seconds,
            nanos: t.nanos,
        })
    }

    /// Convert generated Relationship to aida_gui Relationship
    fn to_gui_relationship(r: &Relationship) -> aida_gui::storage::proto::Relationship {
        aida_gui::storage::proto::Relationship {
            target_id: r.target_id.clone(),
            target_spec_id: r.target_spec_id.clone(),
            rel_type: r.rel_type,
            custom_type_name: r.custom_type_name.clone(),
            created_at: to_gui_timestamp(&r.created_at),
            created_by: r.created_by.clone(),
        }
    }

    /// Convert generated CommentReaction to aida_gui CommentReaction
    fn to_gui_reaction(r: &CommentReaction) -> aida_gui::storage::proto::CommentReaction {
        aida_gui::storage::proto::CommentReaction {
            reaction: r.reaction.clone(),
            author: r.author.clone(),
            added_at: to_gui_timestamp(&r.added_at),
        }
    }

    /// Convert generated Comment to aida_gui Comment
    fn to_gui_comment(c: &Comment) -> aida_gui::storage::proto::Comment {
        aida_gui::storage::proto::Comment {
            id: c.id.clone(),
            content: c.content.clone(),
            author: c.author.clone(),
            created_at: to_gui_timestamp(&c.created_at),
            modified_at: to_gui_timestamp(&c.modified_at),
            parent_id: c.parent_id.clone(),
            reactions: c.reactions.iter().map(to_gui_reaction).collect(),
        }
    }

    /// Convert generated HistoryEntry to aida_gui HistoryEntry
    fn to_gui_history_entry(h: &HistoryEntry) -> aida_gui::storage::proto::HistoryEntry {
        aida_gui::storage::proto::HistoryEntry {
            id: h.id.clone(),
            author: h.author.clone(),
            timestamp: to_gui_timestamp(&h.timestamp),
            changes: h.changes.iter().map(|c| aida_gui::storage::proto::FieldChange {
                field_name: c.field_name.clone(),
                old_value: c.old_value.clone(),
                new_value: c.new_value.clone(),
            }).collect(),
        }
    }

    /// Convert generated UrlLink to aida_gui UrlLink
    fn to_gui_url_link(u: &UrlLink) -> aida_gui::storage::proto::UrlLink {
        aida_gui::storage::proto::UrlLink {
            id: u.id.clone(),
            url: u.url.clone(),
            title: u.title.clone(),
            description: u.description.clone(),
            added_at: to_gui_timestamp(&u.added_at),
            added_by: u.added_by.clone(),
        }
    }

    /// Convert generated Requirement to aida_gui Requirement for UI components
    pub fn to_gui_requirement(req: &Requirement) -> aida_gui::storage::proto::Requirement {
        aida_gui::storage::proto::Requirement {
            id: req.id.clone(),
            spec_id: req.spec_id.clone(),
            prefix_override: req.prefix_override.clone(),
            title: req.title.clone(),
            description: req.description.clone(),
            status: req.status,
            priority: req.priority,
            owner: req.owner.clone(),
            feature: req.feature.clone(),
            created_at: to_gui_timestamp(&req.created_at),
            created_by: req.created_by.clone(),
            modified_at: to_gui_timestamp(&req.modified_at),
            req_type: req.req_type,
            dependency_ids: req.dependency_ids.clone(),
            tags: req.tags.clone(),
            relationships: req.relationships.iter().map(to_gui_relationship).collect(),
            comments: req.comments.iter().map(to_gui_comment).collect(),
            history: req.history.iter().map(to_gui_history_entry).collect(),
            archived: req.archived,
            custom_status: req.custom_status.clone(),
            custom_priority: req.custom_priority.clone(),
            custom_fields: req.custom_fields.clone(),
            urls: req.urls.iter().map(to_gui_url_link).collect(),
        }
    }

    /// Convert Vec<Comment> to aida_gui Comments for comment_list
    pub fn to_gui_comments(comments: &[Comment]) -> Vec<aida_gui::storage::proto::Comment> {
        comments.iter().map(to_gui_comment).collect()
    }
}

// Import shared UI components from aida-gui
// These functions provide consistent rendering between native and web
use aida_gui::ui::{
    // Formatters
    format_status, format_priority, format_type,
    // Badge colors
    status_color, priority_color,
    // List components
    requirement_list_item, ListItemConfig,
    // Form components
    status_combo, priority_combo, type_combo,
    // Comment components
    comment_list, comment_input, CommentInputConfig,
};

/// Connection state to the server
#[derive(Default, Clone)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected(String), // Server version
    Error(String),
}

/// Authentication state
#[derive(Default, Clone)]
pub enum AuthState {
    #[default]
    NotLoggedIn,
    LoggingIn,
    LoggedIn(User), // Logged in user (from generated aida proto)
    Error(String),
}

/// Current view in the application
#[derive(Default, Clone, PartialEq)]
pub enum View {
    #[default]
    Login, // Show login screen first
    List,
    Detail,
    Create,
    Edit,
}

/// Async operation result types
pub enum AsyncResult {
    Store(Result<RequirementsStore, String>),
    Status(Result<GetServerStatusResponse, String>),
    Search(Result<Vec<Requirement>, String>),
    Created(Result<CreateRequirementResponse, String>),
    Updated(Result<Requirement, String>),
    Login(Result<LoginResponse, String>),
}

/// Shared state for async operations
pub struct SharedState {
    pub pending_result: Option<AsyncResult>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            pending_result: None,
        }
    }
}

/// Main AIDA Web Application
pub struct AidaWebApp {
    // Configuration
    server_url: String,

    // Connection state
    connection_state: ConnectionState,

    // Authentication state
    auth_state: AuthState,
    login_identifier: String, // User handle or name for login
    login_pin: String,        // PIN for login
    current_user: Option<User>, // Currently logged in user

    // Data
    requirements: Vec<Requirement>,
    store_metadata: Option<RequirementsStore>,
    selected_idx: Option<usize>,

    // UI state
    current_view: View,
    search_query: String,
    search_results: Option<Vec<Requirement>>,

    // Create/Edit form state
    form_title: String,
    form_description: String,
    form_status: i32,
    form_priority: i32,
    form_type: i32,
    editing_id: Option<String>,

    // Comment form
    new_comment: String,

    // Async state
    shared_state: Rc<RefCell<SharedState>>,
    is_loading: bool,

    // Notification
    notification: Option<(String, f64)>, // (message, expiry_time)
}

impl AidaWebApp {
    /// Create a new application instance
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let server_url = get_server_url().unwrap_or_else(|| "http://localhost:50051".to_string());

        log::info!("AIDA Web App initialized");
        log::info!("Server URL: {}", server_url);

        // Check for saved session in local storage
        let saved_user = load_session_from_storage();
        let (initial_view, auth_state, current_user) = if let Some(user) = saved_user {
            log::info!("Restored session for user: {}", user.name);
            (View::List, AuthState::LoggedIn(user.clone()), Some(user))
        } else {
            (View::Login, AuthState::NotLoggedIn, None)
        };

        let mut app = Self {
            server_url,
            connection_state: ConnectionState::Disconnected,
            auth_state,
            login_identifier: String::new(),
            login_pin: String::new(),
            current_user,
            requirements: Vec::new(),
            store_metadata: None,
            selected_idx: None,
            current_view: initial_view,
            search_query: String::new(),
            search_results: None,
            form_title: String::new(),
            form_description: String::new(),
            form_status: RequirementStatus::Draft.into(),
            form_priority: RequirementPriority::Medium.into(),
            form_type: RequirementType::Functional.into(),
            editing_id: None,
            new_comment: String::new(),
            shared_state: Rc::new(RefCell::new(SharedState::default())),
            is_loading: false,
            notification: None,
        };

        // If logged in, start connection; otherwise wait for login
        if app.current_user.is_some() {
            app.connect_to_server();
        }

        app
    }

    /// Initiate connection to server
    fn connect_to_server(&mut self) {
        self.connection_state = ConnectionState::Connecting;
        self.is_loading = true;

        let server_url = self.server_url.clone();
        let shared_state = self.shared_state.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let mut client = AidaClient::new(&server_url);
            let result = client.get_store().await;
            shared_state.borrow_mut().pending_result = Some(AsyncResult::Store(result));
        });
    }

    /// Attempt login with identifier and PIN
    fn do_login(&mut self) {
        if self.login_identifier.is_empty() {
            self.show_notification("Please enter a handle or name");
            return;
        }

        self.auth_state = AuthState::LoggingIn;
        self.is_loading = true;

        let server_url = self.server_url.clone();
        let identifier = self.login_identifier.clone();
        let pin = self.login_pin.clone();
        let shared_state = self.shared_state.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let mut client = AidaClient::new(&server_url);
            let result = client.login(identifier, pin).await;
            shared_state.borrow_mut().pending_result = Some(AsyncResult::Login(result));
        });
    }

    /// Logout and clear session
    fn logout(&mut self) {
        self.auth_state = AuthState::NotLoggedIn;
        self.current_user = None;
        self.login_identifier.clear();
        self.login_pin.clear();
        self.current_view = View::Login;
        self.requirements.clear();
        self.store_metadata = None;
        self.connection_state = ConnectionState::Disconnected;

        // Clear saved session
        clear_session_from_storage();
        self.show_notification("Logged out");
    }

    /// Perform search
    fn do_search(&mut self) {
        if self.search_query.is_empty() {
            self.search_results = None;
            return;
        }

        self.is_loading = true;
        let server_url = self.server_url.clone();
        let query = self.search_query.clone();
        let shared_state = self.shared_state.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let mut client = AidaClient::new(&server_url);
            let result = client.search(query, true, true, 50).await;
            shared_state.borrow_mut().pending_result = Some(AsyncResult::Search(result));
        });
    }

    /// Create a new requirement
    fn create_requirement(&mut self) {
        self.is_loading = true;
        let server_url = self.server_url.clone();
        let title = self.form_title.clone();
        let description = self.form_description.clone();
        let status = RequirementStatus::try_from(self.form_status).unwrap_or(RequirementStatus::Draft);
        let priority = RequirementPriority::try_from(self.form_priority).unwrap_or(RequirementPriority::Medium);
        let req_type = RequirementType::try_from(self.form_type).unwrap_or(RequirementType::Functional);
        let shared_state = self.shared_state.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let mut client = AidaClient::new(&server_url);
            let result = client
                .create_requirement(title, description, status, priority, req_type, "web-user".to_string())
                .await;
            shared_state.borrow_mut().pending_result = Some(AsyncResult::Created(result));
        });
    }

    /// Update an existing requirement
    fn update_requirement(&mut self) {
        if let Some(id) = &self.editing_id {
            self.is_loading = true;
            let server_url = self.server_url.clone();
            let request = UpdateRequirementRequest {
                id: id.clone(),
                title: Some(self.form_title.clone()),
                description: Some(self.form_description.clone()),
                status: Some(self.form_status),
                priority: Some(self.form_priority),
                req_type: Some(self.form_type),
                owner: None,
                feature: None,
                tags: vec![],
                replace_tags: false,
                archived: None,
                custom_status: None,
                custom_priority: None,
                custom_fields: std::collections::HashMap::new(),
                replace_custom_fields: false,
                modified_by: "web-user".to_string(),
            };
            let shared_state = self.shared_state.clone();

            wasm_bindgen_futures::spawn_local(async move {
                let mut client = AidaClient::new(&server_url);
                let result = client.update_requirement(request).await;
                shared_state.borrow_mut().pending_result = Some(AsyncResult::Updated(result));
            });
        }
    }

    /// Process pending async results
    fn process_async_results(&mut self) {
        let result = self.shared_state.borrow_mut().pending_result.take();

        if let Some(result) = result {
            self.is_loading = false;

            match result {
                AsyncResult::Store(Ok(store)) => {
                    log::info!("Store loaded: {} requirements", store.requirements.len());
                    self.requirements = store.requirements.clone();
                    self.store_metadata = Some(store);
                    self.connection_state = ConnectionState::Connected("Connected".to_string());
                    self.show_notification("Connected to server");
                }
                AsyncResult::Store(Err(e)) => {
                    log::error!("Failed to load store: {}", e);
                    self.connection_state = ConnectionState::Error(e.clone());
                    self.show_notification(&format!("Error: {}", e));
                }
                AsyncResult::Status(Ok(status)) => {
                    self.connection_state = ConnectionState::Connected(status.version);
                }
                AsyncResult::Status(Err(e)) => {
                    self.connection_state = ConnectionState::Error(e);
                }
                AsyncResult::Search(Ok(results)) => {
                    log::info!("Search returned {} results", results.len());
                    self.search_results = Some(results);
                }
                AsyncResult::Search(Err(e)) => {
                    log::error!("Search failed: {}", e);
                    self.show_notification(&format!("Search error: {}", e));
                }
                AsyncResult::Created(Ok(response)) => {
                    log::info!("Created requirement: {}", response.spec_id);
                    if let Some(req) = response.requirement {
                        self.requirements.push(req);
                    }
                    self.clear_form();
                    self.current_view = View::List;
                    self.show_notification(&format!("Created {}", response.spec_id));
                }
                AsyncResult::Created(Err(e)) => {
                    log::error!("Failed to create: {}", e);
                    self.show_notification(&format!("Error: {}", e));
                }
                AsyncResult::Updated(Ok(req)) => {
                    log::info!("Updated requirement: {}", req.spec_id);
                    // Update in list
                    if let Some(idx) = self.requirements.iter().position(|r| r.id == req.id) {
                        self.requirements[idx] = req;
                    }
                    self.clear_form();
                    self.current_view = View::Detail;
                    self.show_notification("Requirement updated");
                }
                AsyncResult::Updated(Err(e)) => {
                    log::error!("Failed to update: {}", e);
                    self.show_notification(&format!("Error: {}", e));
                }
                AsyncResult::Login(Ok(response)) => {
                    if response.success {
                        if let Some(user) = response.user {
                            log::info!("Login successful for: {}", user.name);
                            self.auth_state = AuthState::LoggedIn(user.clone());
                            self.current_user = Some(user.clone());
                            self.login_pin.clear(); // Clear PIN from memory
                            self.current_view = View::List;
                            // Save session to local storage
                            save_session_to_storage(&user);
                            self.show_notification(&format!("Welcome, {}", user.name));
                            // Connect to server after login
                            self.connect_to_server();
                        }
                    } else {
                        log::warn!("Login failed: {}", response.message);
                        self.auth_state = AuthState::Error(response.message.clone());
                        self.show_notification(&format!("Login failed: {}", response.message));
                    }
                }
                AsyncResult::Login(Err(e)) => {
                    log::error!("Login error: {}", e);
                    self.auth_state = AuthState::Error(e.clone());
                    self.show_notification(&format!("Login error: {}", e));
                }
            }
        }
    }

    /// Clear the form fields
    fn clear_form(&mut self) {
        self.form_title.clear();
        self.form_description.clear();
        self.form_status = RequirementStatus::Draft.into();
        self.form_priority = RequirementPriority::Medium.into();
        self.form_type = RequirementType::Functional.into();
        self.editing_id = None;
    }

    /// Populate form from selected requirement
    fn populate_form_from_selection(&mut self) {
        if let Some(idx) = self.selected_idx {
            if let Some(req) = self.requirements.get(idx) {
                self.form_title = req.title.clone();
                self.form_description = req.description.clone();
                self.form_status = req.status;
                self.form_priority = req.priority;
                self.form_type = req.req_type;
                self.editing_id = Some(req.id.clone());
            }
        }
    }

    /// Show a temporary notification
    fn show_notification(&mut self, message: &str) {
        // Set expiry to 3 seconds from now (using frame time approximation)
        self.notification = Some((message.to_string(), 3.0));
    }

    /// Draw the top panel with connection status
    fn draw_top_panel(&mut self, ctx: &egui::Context) {
        let mut should_logout = false;

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("AIDA Web").strong());
                ui.separator();

                // Connection status
                match &self.connection_state {
                    ConnectionState::Connected(version) => {
                        ui.colored_label(Color32::from_rgb(100, 200, 100), "● Connected");
                        ui.label(RichText::new(version).small().weak());
                    }
                    ConnectionState::Connecting => {
                        ui.spinner();
                        ui.label("Connecting...");
                    }
                    ConnectionState::Error(e) => {
                        ui.colored_label(Color32::from_rgb(200, 100, 100), "● Error");
                        ui.label(RichText::new(e).small().weak());
                        if ui.button("Retry").clicked() {
                            self.connect_to_server();
                        }
                    }
                    ConnectionState::Disconnected => {
                        ui.label("○ Disconnected");
                        if ui.button("Connect").clicked() {
                            self.connect_to_server();
                        }
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // User info and logout
                    if let Some(ref user) = self.current_user {
                        if ui.button("Logout").clicked() {
                            should_logout = true;
                        }
                        ui.label(RichText::new(&user.name).strong());
                        ui.label("@");
                        ui.label(RichText::new(&user.handle).monospace());
                        ui.separator();
                    }

                    ui.label(format!("{} requirements", self.requirements.len()));

                    if self.is_loading {
                        ui.spinner();
                    }
                });
            });

            // Notification bar
            if let Some((msg, _)) = &self.notification {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(100, 180, 255), format!("ℹ {}", msg));
                });
            }
        });

        // Handle logout after UI borrowing is done
        if should_logout {
            self.logout();
        }
    }

    /// Draw the left panel with requirements list
    fn draw_left_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("left_panel")
            .default_width(280.0)
            .width_range(200.0..=400.0)
            .resizable(true)
            .show(ctx, |ui| {
                // Search bar
                ui.horizontal(|ui| {
                    let response = ui.add(
                        TextEdit::singleline(&mut self.search_query)
                            .hint_text("Search...")
                            .desired_width(ui.available_width() - 60.0),
                    );
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.do_search();
                    }
                    if ui.button("🔍").clicked() {
                        self.do_search();
                    }
                    if ui.button("✕").clicked() {
                        self.search_query.clear();
                        self.search_results = None;
                    }
                });

                ui.separator();

                // Action buttons
                ui.horizontal(|ui| {
                    if ui.button("➕ New").clicked() {
                        self.clear_form();
                        self.current_view = View::Create;
                    }
                    if ui.button("🔄 Refresh").clicked() {
                        self.connect_to_server();
                    }
                });

                ui.separator();

                // Requirements list
                let items = if let Some(results) = &self.search_results {
                    results.as_slice()
                } else {
                    self.requirements.as_slice()
                };

                if self.search_results.is_some() {
                    ui.label(RichText::new(format!("Search Results ({})", items.len())).small());
                    ui.separator();
                }

                ScrollArea::vertical().show(ui, |ui| {
                    let list_config = ListItemConfig::with_title();
                    for (idx, req) in items.iter().enumerate() {
                        let is_selected = self.selected_idx == Some(idx) && self.search_results.is_none();
                        let is_search_selected = self.search_results.is_some()
                            && self.selected_idx.is_some()
                            && self.requirements.get(self.selected_idx.unwrap()).map(|r| &r.id) == Some(&req.id);

                        let selected = is_selected || is_search_selected;

                        // Use shared list item component (convert to aida_gui proto type)
                        if requirement_list_item(ui, &gui_convert::to_gui_requirement(req), selected, &list_config) {
                            // Find the actual index in requirements list
                            if let Some(actual_idx) = self.requirements.iter().position(|r| r.id == req.id) {
                                self.selected_idx = Some(actual_idx);
                                self.current_view = View::Detail;
                            }
                        }
                    }
                });
            });
    }

    /// Draw the central panel with detail/create/edit views
    fn draw_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.current_view {
                View::Login => {
                    self.draw_login_view(ui);
                }
                View::List => {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Select a requirement from the list").weak());
                    });
                }
                View::Detail => {
                    self.draw_detail_view(ui);
                }
                View::Create => {
                    self.draw_create_view(ui);
                }
                View::Edit => {
                    self.draw_edit_view(ui);
                }
            }
        });
    }

    /// Draw the login view
    fn draw_login_view(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);

            ui.heading(RichText::new("AIDA").size(48.0).strong());
            ui.label(RichText::new("AI Design Assistant").weak());

            ui.add_space(40.0);

            // Login form
            ui.group(|ui| {
                ui.set_max_width(300.0);
                ui.vertical_centered(|ui| {
                    ui.heading("Login");
                    ui.add_space(16.0);

                    // Error message
                    if let AuthState::Error(ref msg) = self.auth_state {
                        ui.colored_label(Color32::from_rgb(255, 100, 100), msg);
                        ui.add_space(8.0);
                    }

                    // Handle/Name field
                    ui.horizontal(|ui| {
                        ui.label("Handle:");
                        ui.add_space(8.0);
                    });
                    let handle_response = ui.add(
                        TextEdit::singleline(&mut self.login_identifier)
                            .hint_text("Enter handle or name")
                            .desired_width(200.0),
                    );

                    ui.add_space(8.0);

                    // PIN field
                    ui.horizontal(|ui| {
                        ui.label("PIN:");
                        ui.add_space(8.0);
                    });
                    let pin_response = ui.add(
                        TextEdit::singleline(&mut self.login_pin)
                            .password(true)
                            .hint_text("Enter PIN (optional)")
                            .desired_width(200.0),
                    );

                    ui.add_space(16.0);

                    // Login button
                    let is_logging_in = matches!(self.auth_state, AuthState::LoggingIn);
                    ui.add_enabled_ui(!is_logging_in, |ui| {
                        if is_logging_in {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Logging in...");
                            });
                        } else {
                            let enter_pressed = (handle_response.lost_focus() || pin_response.lost_focus())
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));

                            if ui.button(RichText::new("Login").size(16.0)).clicked() || enter_pressed {
                                self.do_login();
                            }
                        }
                    });

                    ui.add_space(8.0);
                    ui.label(RichText::new("Enter your handle or name to login.").small().weak());
                    ui.label(RichText::new("PIN is optional for users without one set.").small().weak());
                });
            });

            ui.add_space(20.0);
            ui.label(RichText::new(format!("Server: {}", self.server_url)).small().weak());
        });
    }

    /// Draw the detail view for selected requirement
    fn draw_detail_view(&mut self, ui: &mut Ui) {
        let mut edit_clicked = false;

        if let Some(idx) = self.selected_idx {
            if let Some(req) = self.requirements.get(idx).cloned() {
                ScrollArea::vertical().show(ui, |ui| {
                    // Header
                    ui.horizontal(|ui| {
                        ui.heading(RichText::new(&req.spec_id).monospace());

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✏️ Edit").clicked() {
                                edit_clicked = true;
                            }
                        });
                    });

                    ui.separator();

                    // Status and priority badges
                    ui.horizontal(|ui| {
                        let status_text = format_status(req.status);
                        let status_color = status_color(req.status);
                        ui.label(RichText::new(status_text).background_color(status_color).strong());

                        let priority_text = format_priority(req.priority);
                        let priority_color = priority_color(req.priority);
                        ui.label(RichText::new(priority_text).background_color(priority_color));

                        let type_text = format_type(req.req_type);
                        ui.label(RichText::new(type_text).weak());
                    });

                    ui.add_space(8.0);

                    // Title
                    ui.heading(&req.title);

                    ui.add_space(8.0);

                    // Description
                    if !req.description.is_empty() {
                        ui.label(&req.description);
                    } else {
                        ui.label(RichText::new("No description").weak().italics());
                    }

                    ui.add_space(16.0);
                    ui.separator();

                    // Metadata
                    egui::CollapsingHeader::new("Details").default_open(false).show(ui, |ui| {
                        egui::Grid::new("detail_grid").num_columns(2).show(ui, |ui| {
                            ui.label("ID:");
                            ui.label(RichText::new(&req.id).monospace().small());
                            ui.end_row();

                            ui.label("Owner:");
                            ui.label(if req.owner.is_empty() { "—" } else { &req.owner });
                            ui.end_row();

                            ui.label("Feature:");
                            ui.label(if req.feature.is_empty() { "—" } else { &req.feature });
                            ui.end_row();

                            if !req.tags.is_empty() {
                                ui.label("Tags:");
                                ui.label(req.tags.join(", "));
                                ui.end_row();
                            }
                        });
                    });

                    // Comments section - using shared components
                    ui.add_space(16.0);
                    egui::CollapsingHeader::new(format!("Comments ({})", req.comments.len()))
                        .default_open(true)
                        .show(ui, |ui| {
                            // Use shared comment list component (convert to aida_gui proto type)
                            let gui_comments = gui_convert::to_gui_comments(&req.comments);
                            comment_list(ui, &gui_comments);
                        });
                });

                // Comment input outside the ScrollArea to avoid borrow conflicts
                // Use shared comment input component
                ui.separator();
                if let Some(content) = comment_input(ui, &mut self.new_comment, &CommentInputConfig::default()) {
                    // TODO: Add comment via client - for now just log
                    log::info!("New comment: {}", content);
                }
            }
        }

        // Handle edit button click after the borrow is released
        if edit_clicked {
            self.populate_form_from_selection();
            self.current_view = View::Edit;
        }
    }

    /// Draw the create requirement view
    fn draw_create_view(&mut self, ui: &mut Ui) {
        ui.heading("Create Requirement");
        ui.separator();

        self.draw_requirement_form(ui);

        ui.add_space(16.0);
        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Create").clicked() {
                if !self.form_title.is_empty() {
                    self.create_requirement();
                }
            }
            if ui.button("Cancel").clicked() {
                self.clear_form();
                self.current_view = View::List;
            }
        });
    }

    /// Draw the edit requirement view
    fn draw_edit_view(&mut self, ui: &mut Ui) {
        ui.heading("Edit Requirement");
        ui.separator();

        self.draw_requirement_form(ui);

        ui.add_space(16.0);
        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                self.update_requirement();
            }
            if ui.button("Cancel").clicked() {
                self.clear_form();
                self.current_view = View::Detail;
            }
        });
    }

    /// Draw the requirement form (used for create and edit)
    /// Uses shared combo box components for consistent rendering with native GUI
    fn draw_requirement_form(&mut self, ui: &mut Ui) {
        egui::Grid::new("form_grid")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                ui.label("Title:");
                ui.add(
                    TextEdit::singleline(&mut self.form_title)
                        .hint_text("Requirement title")
                        .desired_width(400.0),
                );
                ui.end_row();

                ui.label("Description:");
                ui.add(
                    TextEdit::multiline(&mut self.form_description)
                        .hint_text("Detailed description...")
                        .desired_width(400.0)
                        .desired_rows(6),
                );
                ui.end_row();

                // Use shared combo box components
                ui.label("Status:");
                status_combo(ui, &mut self.form_status);
                ui.end_row();

                ui.label("Priority:");
                priority_combo(ui, &mut self.form_priority);
                ui.end_row();

                ui.label("Type:");
                type_combo(ui, &mut self.form_type);
                ui.end_row();
            });
    }
}

impl eframe::App for AidaWebApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Process any pending async results
        self.process_async_results();

        // Update notification timer
        if let Some((_, ref mut expiry)) = self.notification {
            *expiry -= ctx.input(|i| i.stable_dt) as f64;
            if *expiry <= 0.0 {
                self.notification = None;
            }
        }

        // Request continuous repaint while loading or notification showing
        if self.is_loading || self.notification.is_some() {
            ctx.request_repaint();
        }

        // Draw UI
        // Only show top panel and left panel when logged in
        if self.current_view != View::Login {
            self.draw_top_panel(ctx);
            self.draw_left_panel(ctx);
        }
        self.draw_central_panel(ctx);
    }
}

// Helper functions

fn get_server_url() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let window = web_sys::window()?;
        let config = js_sys::Reflect::get(&window, &"AIDA_CONFIG".into()).ok()?;
        let server_url = js_sys::Reflect::get(&config, &"serverUrl".into()).ok()?;
        server_url.as_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

// Local helper functions (format_status, format_priority, etc.) have been moved to
// the shared aida_gui::ui module for code reuse between native and web builds

// Session storage constants
const SESSION_STORAGE_KEY: &str = "aida_session";

/// Load user session from browser local storage
/// trace:AUTH-0001 | ai:claude:high
fn load_session_from_storage() -> Option<User> {
    #[cfg(target_arch = "wasm32")]
    {
        let window = web_sys::window()?;
        let storage = window.local_storage().ok()??;
        let json = storage.get_item(SESSION_STORAGE_KEY).ok()??;

        // Parse stored JSON to User (generated proto User with has_pin field)
        let parsed: serde_json::Value = serde_json::from_str(&json).ok()?;
        Some(User {
            id: parsed.get("id")?.as_str()?.to_string(),
            spec_id: parsed.get("spec_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            name: parsed.get("name")?.as_str()?.to_string(),
            email: parsed.get("email").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            handle: parsed.get("handle")?.as_str()?.to_string(),
            has_pin: parsed.get("has_pin").and_then(|v| v.as_bool()).unwrap_or(false),
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// Save user session to browser local storage
/// trace:AUTH-0001 | ai:claude:high
fn save_session_to_storage(user: &User) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                // Serialize user to JSON
                let json = serde_json::json!({
                    "id": user.id,
                    "spec_id": user.spec_id,
                    "name": user.name,
                    "email": user.email,
                    "handle": user.handle,
                    "has_pin": user.has_pin,
                });
                if let Ok(json_str) = serde_json::to_string(&json) {
                    let _ = storage.set_item(SESSION_STORAGE_KEY, &json_str);
                    log::debug!("Session saved for user: {}", user.name);
                }
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = user;
    }
}

/// Clear user session from browser local storage
/// trace:AUTH-0001 | ai:claude:high
fn clear_session_from_storage() {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                let _ = storage.remove_item(SESSION_STORAGE_KEY);
                log::debug!("Session cleared");
            }
        }
    }
}
