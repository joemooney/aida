// trace:TASK-3428 | ai:codex
use crate::role::{assemble_role_picker_items, RolePickTarget};
mod task_1239_picker_tests {
    use super::*;

    #[test]
    fn stakeholder_personas_append_as_a_separate_labeled_section() {
        // Two build/driver roles come first; guest + requester follow under a
        // divider heading, each tagged "not a build seat". trace:TASK-1239
        let role_labels = vec!["  advisor".to_string(), "  implementer".to_string()];
        let role_names = vec!["advisor".to_string(), "implementer".to_string()];
        let (labels, targets) = assemble_role_picker_items(role_labels, &role_names, None);

        // Build roles stay first and map to Role targets.
        assert_eq!(targets[0], RolePickTarget::Role("advisor".to_string()));
        assert_eq!(targets[1], RolePickTarget::Role("implementer".to_string()));

        // Then a non-selectable section heading.
        assert_eq!(targets[2], RolePickTarget::Divider);
        assert!(labels[2].contains("Stakeholder personas"), "{}", labels[2]);
        assert!(labels[2].contains("not a build seat"), "{}", labels[2]);

        // Then the two personas, launchable, visually tagged non-build.
        assert_eq!(targets[3], RolePickTarget::Persona("guest".to_string()));
        assert_eq!(targets[4], RolePickTarget::Persona("requester".to_string()));
        assert!(labels[3].contains("guest") && labels[3].contains("not a build seat"));
        assert!(labels[4].contains("requester") && labels[4].contains("not a build seat"));

        // The personas are appended, never mixed into the driver roles.
        assert_eq!(labels.len(), 5);
        assert_eq!(targets.len(), 5);
    }

    #[test]
    fn active_stakeholder_persona_is_marked() {
        let (labels, _) = assemble_role_picker_items(
            vec!["  advisor".to_string()],
            &["advisor".to_string()],
            Some("guest"),
        );
        // guest is index 2 (role, divider, guest) and carries the active `*`.
        assert!(labels[2].starts_with("*"), "{}", labels[2]);
    }
}
