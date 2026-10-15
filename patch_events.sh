sed -i 's/std::env::var("AIDA_SESSION_ROLE")/crate::seat_authority::current_seat(\&crate::find_project_root().unwrap_or_default()).ok_or("")/g' aida-cli-lib/src/events.rs
