// trace:FR-0227 | ai:claude:high
//! Conversion utilities between protobuf types and aida-core types

use chrono::{DateTime, Utc};

use aida_core::{
    AiActionPromptConfig, AiPromptConfig, AiTypePromptConfig, Comment, CommentReaction,
    CustomFieldDefinition, CustomTypeDefinition, FeatureDefinition, FieldChange, HistoryEntry,
    IdConfiguration, IdFormat, NumberingStrategy, ReactionDefinition, Relationship,
    RelationshipDefinition, RelationshipType as CoreRelType, Requirement,
    RequirementPriority as CorePriority, RequirementStatus as CoreStatus,
    RequirementType as CoreReqType, RequirementsStore, Team, UrlLink, User,
};

use crate::proto;

// ============================================================================
// Timestamp conversions
// ============================================================================

pub fn datetime_to_proto(dt: DateTime<Utc>) -> proto::Timestamp {
    proto::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

// ============================================================================
// Status conversions
// ============================================================================

pub fn status_to_proto(status: &CoreStatus) -> proto::RequirementStatus {
    match status {
        CoreStatus::Draft => proto::RequirementStatus::Draft,
        CoreStatus::Approved => proto::RequirementStatus::Approved,
        CoreStatus::Planned => proto::RequirementStatus::Planned,
        CoreStatus::InProgress => proto::RequirementStatus::InProgress,
        CoreStatus::Done => proto::RequirementStatus::Done,
        CoreStatus::Completed => proto::RequirementStatus::Completed,
        CoreStatus::Rejected => proto::RequirementStatus::Rejected,
        CoreStatus::NeedsAttention => proto::RequirementStatus::NeedsAttention,
    }
}

pub fn proto_to_status(status: proto::RequirementStatus) -> CoreStatus {
    match status {
        proto::RequirementStatus::Draft => CoreStatus::Draft,
        proto::RequirementStatus::Approved => CoreStatus::Approved,
        proto::RequirementStatus::Planned => CoreStatus::Planned,
        proto::RequirementStatus::InProgress => CoreStatus::InProgress,
        proto::RequirementStatus::Done => CoreStatus::Done,
        proto::RequirementStatus::Completed => CoreStatus::Completed,
        proto::RequirementStatus::Rejected => CoreStatus::Rejected,
        proto::RequirementStatus::NeedsAttention => CoreStatus::NeedsAttention,
        proto::RequirementStatus::Unspecified => CoreStatus::Draft,
    }
}

// ============================================================================
// Priority conversions
// ============================================================================

pub fn priority_to_proto(priority: &CorePriority) -> proto::RequirementPriority {
    match priority {
        CorePriority::High => proto::RequirementPriority::High,
        CorePriority::Medium => proto::RequirementPriority::Medium,
        CorePriority::Low => proto::RequirementPriority::Low,
    }
}

pub fn proto_to_priority(priority: proto::RequirementPriority) -> CorePriority {
    match priority {
        proto::RequirementPriority::High => CorePriority::High,
        proto::RequirementPriority::Medium => CorePriority::Medium,
        proto::RequirementPriority::Low => CorePriority::Low,
        proto::RequirementPriority::Unspecified => CorePriority::Medium,
    }
}

// ============================================================================
// Type conversions
// ============================================================================

pub fn req_type_to_proto(req_type: &CoreReqType) -> proto::RequirementType {
    match req_type {
        CoreReqType::Functional => proto::RequirementType::Functional,
        CoreReqType::NonFunctional => proto::RequirementType::NonFunctional,
        CoreReqType::System => proto::RequirementType::System,
        CoreReqType::User => proto::RequirementType::User,
        CoreReqType::ChangeRequest => proto::RequirementType::ChangeRequest,
        CoreReqType::Bug => proto::RequirementType::Bug,
        CoreReqType::Epic => proto::RequirementType::Epic,
        CoreReqType::Story => proto::RequirementType::Story,
        CoreReqType::Task => proto::RequirementType::Task,
        CoreReqType::Spike => proto::RequirementType::Spike,
        CoreReqType::Sprint => proto::RequirementType::Sprint,
        CoreReqType::Folder => proto::RequirementType::Folder,
        CoreReqType::Meta => proto::RequirementType::Meta,
        // Docs-layer entity types (FR-1-074, STORY-104) — gRPC proto doesn't
        // yet expose dedicated variants, so they project to Meta for
        // transport. Update when the proto is regenerated with the new tags.
        CoreReqType::Principle
        | CoreReqType::Vision
        | CoreReqType::Constraint
        | CoreReqType::Decision
        | CoreReqType::Term
        | CoreReqType::Doc => proto::RequirementType::Meta,
    }
}

pub fn proto_to_req_type(req_type: proto::RequirementType) -> CoreReqType {
    match req_type {
        proto::RequirementType::Functional => CoreReqType::Functional,
        proto::RequirementType::NonFunctional => CoreReqType::NonFunctional,
        proto::RequirementType::System => CoreReqType::System,
        proto::RequirementType::User => CoreReqType::User,
        proto::RequirementType::ChangeRequest => CoreReqType::ChangeRequest,
        proto::RequirementType::Bug => CoreReqType::Bug,
        proto::RequirementType::Epic => CoreReqType::Epic,
        proto::RequirementType::Story => CoreReqType::Story,
        proto::RequirementType::Task => CoreReqType::Task,
        proto::RequirementType::Spike => CoreReqType::Spike,
        proto::RequirementType::Sprint => CoreReqType::Sprint,
        proto::RequirementType::Folder => CoreReqType::Folder,
        proto::RequirementType::Meta => CoreReqType::Meta,
        proto::RequirementType::Unspecified => CoreReqType::Functional,
    }
}

// ============================================================================
// Relationship type conversions
// ============================================================================

pub fn rel_type_to_proto(rel_type: &CoreRelType) -> (proto::RelationshipType, String) {
    match rel_type {
        CoreRelType::Parent => (proto::RelationshipType::Parent, String::new()),
        CoreRelType::Child => (proto::RelationshipType::Child, String::new()),
        CoreRelType::Duplicate => (proto::RelationshipType::Duplicate, String::new()),
        CoreRelType::Verifies => (proto::RelationshipType::Verifies, String::new()),
        CoreRelType::VerifiedBy => (proto::RelationshipType::VerifiedBy, String::new()),
        CoreRelType::References => (proto::RelationshipType::References, String::new()),
        // STORY-333: proto schema has no dedicated BlockedBy/Blocks
        // variants yet — wire them as Custom-with-name so existing gRPC
        // clients keep working. The string forms round-trip back to the
        // typed variants via `RelationshipType::from_str` on the receive
        // side. trace:STORY-333 | ai:claude
        CoreRelType::BlockedBy => (proto::RelationshipType::Custom, "blocked-by".to_string()),
        CoreRelType::Blocks => (proto::RelationshipType::Custom, "blocks".to_string()),
        CoreRelType::Custom(name) => (proto::RelationshipType::Custom, name.clone()),
    }
}

pub fn proto_to_rel_type(rel_type: proto::RelationshipType, custom_name: &str) -> CoreRelType {
    match rel_type {
        proto::RelationshipType::Parent => CoreRelType::Parent,
        proto::RelationshipType::Child => CoreRelType::Child,
        proto::RelationshipType::Duplicate => CoreRelType::Duplicate,
        proto::RelationshipType::Verifies => CoreRelType::Verifies,
        proto::RelationshipType::VerifiedBy => CoreRelType::VerifiedBy,
        proto::RelationshipType::References => CoreRelType::References,
        // STORY-333: route Custom-with-name through `from_str` so the
        // typed `blocked-by` / `blocks` strings sent over the wire land
        // as the typed variants on the receive side. Unknown custom
        // names still fall through to `Custom(name)`.
        // trace:STORY-333 | ai:claude
        proto::RelationshipType::Custom => CoreRelType::from_str(custom_name),
        proto::RelationshipType::Unspecified => CoreRelType::References,
    }
}

// ============================================================================
// Relationship conversions
// ============================================================================

pub fn relationship_to_proto(rel: &Relationship) -> proto::Relationship {
    let (rel_type, custom_name) = rel_type_to_proto(&rel.rel_type);
    proto::Relationship {
        target_id: rel.target_id.to_string(),
        target_spec_id: String::new(), // The core Relationship struct doesn't have this field
        rel_type: rel_type.into(),
        custom_type_name: custom_name,
        created_at: rel.created_at.map(datetime_to_proto),
        created_by: rel.created_by.clone().unwrap_or_default(),
    }
}

// ============================================================================
// Comment conversions
// ============================================================================

pub fn comment_to_proto(comment: &Comment) -> proto::Comment {
    proto::Comment {
        id: comment.id.to_string(),
        content: comment.content.clone(),
        author: comment.author.clone(),
        created_at: Some(datetime_to_proto(comment.created_at)),
        modified_at: Some(datetime_to_proto(comment.modified_at)),
        parent_id: comment
            .parent_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        reactions: comment.reactions.iter().map(reaction_to_proto).collect(),
        // Note: replies are nested in the core model but flattened in proto
    }
}

// ============================================================================
// Reaction conversions
// ============================================================================

pub fn reaction_to_proto(reaction: &CommentReaction) -> proto::CommentReaction {
    proto::CommentReaction {
        reaction: reaction.reaction.clone(),
        author: reaction.author.clone(),
        added_at: Some(datetime_to_proto(reaction.added_at)),
    }
}

// ============================================================================
// History entry conversions
// ============================================================================

pub fn history_to_proto(entry: &HistoryEntry) -> proto::HistoryEntry {
    proto::HistoryEntry {
        id: entry.id.to_string(),
        author: entry.author.clone(),
        timestamp: Some(datetime_to_proto(entry.timestamp)),
        changes: entry.changes.iter().map(field_change_to_proto).collect(),
    }
}

pub fn field_change_to_proto(change: &FieldChange) -> proto::FieldChange {
    proto::FieldChange {
        field_name: change.field_name.clone(),
        old_value: change.old_value.clone(),
        new_value: change.new_value.clone(),
    }
}

// ============================================================================
// URL Link conversions
// ============================================================================

pub fn url_link_to_proto(link: &UrlLink) -> proto::UrlLink {
    proto::UrlLink {
        id: link.id.to_string(),
        url: link.url.clone(),
        title: link.title.clone(),
        description: link.description.clone().unwrap_or_default(),
        added_at: Some(datetime_to_proto(link.added_at)),
        added_by: link.added_by.clone(),
        open_mode: url_open_mode_to_proto(&link.open_mode),
    }
}

pub fn url_open_mode_to_proto(mode: &aida_core::UrlOpenMode) -> i32 {
    match mode {
        aida_core::UrlOpenMode::Preview => 0,
        aida_core::UrlOpenMode::NewTab => 1,
    }
}

// ============================================================================
// Requirement conversions
// ============================================================================

pub fn requirement_to_proto(req: &Requirement) -> proto::Requirement {
    proto::Requirement {
        id: req.id.to_string(),
        spec_id: req.spec_id.clone().unwrap_or_default(),
        prefix_override: req.prefix_override.clone().unwrap_or_default(),
        title: req.title.clone(),
        description: req.description.clone(),
        status: status_to_proto(&req.status).into(),
        priority: priority_to_proto(&req.priority).into(),
        owner: req.owner.clone(),
        feature: req.feature.clone(),
        created_at: Some(datetime_to_proto(req.created_at)),
        created_by: req.created_by.clone().unwrap_or_default(),
        modified_at: Some(datetime_to_proto(req.modified_at)),
        req_type: req_type_to_proto(&req.req_type).into(),
        dependency_ids: req.dependencies.iter().map(|id| id.to_string()).collect(),
        tags: req.tags.iter().cloned().collect(),
        relationships: req
            .relationships
            .iter()
            .map(relationship_to_proto)
            .collect(),
        comments: req.comments.iter().map(comment_to_proto).collect(),
        history: req.history.iter().map(history_to_proto).collect(),
        archived: req.archived,
        custom_status: req.custom_status.clone().unwrap_or_default(),
        custom_priority: req.custom_priority.clone().unwrap_or_default(),
        custom_fields: req.custom_fields.clone(),
        urls: req.urls.iter().map(url_link_to_proto).collect(),
        agreed_id: req.agreed_id.clone().unwrap_or_default(),
    }
}

// ============================================================================
// Feature definition conversions
// ============================================================================

pub fn feature_to_proto(feature: &FeatureDefinition) -> proto::FeatureDefinition {
    proto::FeatureDefinition {
        name: feature.name.clone(),
        prefix: feature.prefix.clone(),
        number: feature.number as i32,
    }
}

// ============================================================================
// User conversions
// ============================================================================

pub fn user_to_proto(user: &User) -> proto::User {
    proto::User {
        id: user.id.to_string(),
        spec_id: user.spec_id.clone().unwrap_or_default(),
        name: user.name.clone(),
        email: user.email.clone(),
        handle: user.handle.clone(),
        has_pin: user.has_pin(),
    }
}

// ============================================================================
// ID configuration conversions
// ============================================================================

pub fn id_config_to_proto(config: &IdConfiguration) -> proto::IdConfiguration {
    proto::IdConfiguration {
        format: match config.format {
            IdFormat::SingleLevel => "single_level".to_string(),
            IdFormat::TwoLevel => "two_level".to_string(),
        },
        numbering: match config.numbering {
            NumberingStrategy::Global => "global".to_string(),
            NumberingStrategy::PerPrefix => "per_prefix".to_string(),
            NumberingStrategy::PerFeatureType => "per_feature_type".to_string(),
        },
        digits: config.digits as i32,
    }
}

// ============================================================================
// Team conversions
// ============================================================================

pub fn team_to_proto(team: &Team) -> proto::Team {
    proto::Team {
        id: team.id.to_string(),
        spec_id: team.spec_id.clone().unwrap_or_default(),
        name: team.name.clone(),
        description: team.description.clone(),
        member_ids: team.member_ids.iter().map(|id| id.to_string()).collect(),
    }
}

// ============================================================================
// Reaction definition conversions
// ============================================================================

pub fn reaction_def_to_proto(def: &ReactionDefinition) -> proto::ReactionDefinition {
    proto::ReactionDefinition {
        name: def.name.clone(),
        emoji: def.emoji.clone(),
        label: def.label.clone(),
        description: def.description.clone().unwrap_or_default(),
        built_in: def.built_in,
    }
}

// ============================================================================
// Custom field definition conversions
// ============================================================================

pub fn custom_field_to_proto(field: &CustomFieldDefinition) -> proto::CustomFieldDefinition {
    proto::CustomFieldDefinition {
        name: field.name.clone(),
        label: field.label.clone(),
        field_type: field.field_type.to_string(),
        required: field.required,
        options: field.options.clone(),
        description: field.description.clone().unwrap_or_default(),
        order: field.order,
    }
}

// ============================================================================
// Custom type definition conversions
// ============================================================================

pub fn type_def_to_proto(def: &CustomTypeDefinition) -> proto::CustomTypeDefinition {
    proto::CustomTypeDefinition {
        name: def.name.clone(),
        display_name: def.display_name.clone(),
        description: def.description.clone().unwrap_or_default(),
        prefix: def.prefix.clone().unwrap_or_default(),
        statuses: def.statuses.clone(),
        custom_fields: def
            .custom_fields
            .iter()
            .map(custom_field_to_proto)
            .collect(),
        built_in: def.built_in,
        color: def.color.clone().unwrap_or_default(),
        stateless: def.stateless,
    }
}

// ============================================================================
// Relationship definition conversions
// ============================================================================

pub fn rel_def_to_proto(def: &RelationshipDefinition) -> proto::RelationshipDefinitionProto {
    proto::RelationshipDefinitionProto {
        name: def.name.clone(),
        display_name: def.display_name.clone(),
        description: def.description.clone(),
        inverse: def.inverse.clone().unwrap_or_default(),
        symmetric: def.symmetric,
        cardinality: format!("{:?}", def.cardinality),
        source_types: def.source_types.clone(),
        target_types: def.target_types.clone(),
        built_in: def.built_in,
        color: def.color.clone().unwrap_or_default(),
        icon: def.icon.clone().unwrap_or_default(),
    }
}

// ============================================================================
// AI prompt config conversions
// ============================================================================

pub fn ai_action_to_proto(action: &AiActionPromptConfig) -> proto::AiActionConfig {
    proto::AiActionConfig {
        additional_instructions: action.additional_instructions.clone(),
    }
}

pub fn type_prompt_to_proto(tp: &AiTypePromptConfig) -> proto::TypePromptConfig {
    proto::TypePromptConfig {
        type_name: tp.type_name.clone(),
        // The proto uses shorter names than the core type
        evaluation: tp.evaluation_extra.clone(),
        improve: tp.improve_extra.clone(),
        generate_children: String::new(), // Core type doesn't have this field
        generate_children_extra: tp.generate_children_extra.clone(),
    }
}

pub fn ai_prompts_to_proto(config: &AiPromptConfig) -> proto::AiPromptConfig {
    proto::AiPromptConfig {
        global_context: config.global_context.clone(),
        evaluation: Some(ai_action_to_proto(&config.evaluation)),
        duplicates: Some(ai_action_to_proto(&config.duplicates)),
        relationships: Some(ai_action_to_proto(&config.relationships)),
        improve: Some(ai_action_to_proto(&config.improve)),
        generate_children: Some(ai_action_to_proto(&config.generate_children)),
        type_prompts: config
            .type_prompts
            .iter()
            .map(type_prompt_to_proto)
            .collect(),
    }
}

// ============================================================================
// Store conversions
// ============================================================================

pub fn store_to_proto(store: &RequirementsStore) -> proto::RequirementsStore {
    proto::RequirementsStore {
        name: store.name.clone(),
        title: store.title.clone(),
        description: store.description.clone(),
        requirements: store
            .requirements
            .iter()
            .map(requirement_to_proto)
            .collect(),
        users: store.users.iter().map(user_to_proto).collect(),
        features: store.features.iter().map(feature_to_proto).collect(),
        id_config: Some(id_config_to_proto(&store.id_config)),
        next_spec_number: store.next_spec_number as i32,
        prefix_counters: store
            .prefix_counters
            .iter()
            .map(|(k, v)| (k.clone(), *v as i32))
            .collect(),
        // Extended metadata fields
        relationship_definitions: store
            .relationship_definitions
            .iter()
            .map(rel_def_to_proto)
            .collect(),
        reaction_definitions: store
            .reaction_definitions
            .iter()
            .map(reaction_def_to_proto)
            .collect(),
        type_definitions: store
            .type_definitions
            .iter()
            .map(type_def_to_proto)
            .collect(),
        allowed_prefixes: store.allowed_prefixes.clone(),
        restrict_prefixes: store.restrict_prefixes,
        ai_prompts: Some(ai_prompts_to_proto(&store.ai_prompts)),
        teams: store.teams.iter().map(team_to_proto).collect(),
    }
}
