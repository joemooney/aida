use super::validate_session_start_args;

#[test]
fn test_valid_start_args() {
    // Only --owns is fine
    assert!(validate_session_start_args(&[
        "aida".to_string(),
        "session".to_string(),
        "start".to_string(),
        "--owns".to_string(),
        "BUG-360".to_string()
    ])
    .is_ok());

    // Only --spec is fine
    assert!(validate_session_start_args(&[
        "aida".to_string(),
        "session".to_string(),
        "start".to_string(),
        "--spec".to_string(),
        "BUG-360".to_string()
    ])
    .is_ok());

    // Only --owns= is fine
    assert!(validate_session_start_args(&[
        "aida".to_string(),
        "session".to_string(),
        "start".to_string(),
        "--owns=BUG-360".to_string()
    ])
    .is_ok());

    // Only --spec= is fine
    assert!(validate_session_start_args(&[
        "aida".to_string(),
        "session".to_string(),
        "start".to_string(),
        "--spec=BUG-360".to_string()
    ])
    .is_ok());
}

#[test]
fn test_conflicting_start_args() {
    // Both --owns and --spec conflicts
    assert!(validate_session_start_args(&[
        "aida".to_string(),
        "session".to_string(),
        "start".to_string(),
        "--owns".to_string(),
        "BUG-360".to_string(),
        "--spec".to_string(),
        "BUG-360".to_string()
    ])
    .is_err());

    // Both --owns= and --spec= conflicts
    assert!(validate_session_start_args(&[
        "aida".to_string(),
        "session".to_string(),
        "start".to_string(),
        "--owns=BUG-360".to_string(),
        "--spec=BUG-360".to_string()
    ])
    .is_err());
}
