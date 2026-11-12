pub(crate) fn print_merge_hold_no_forge(json: bool) {
    if json {
        println!("{}", serde_json::json!({"status":"no_forge", "labels":{}}));
    } else {
        println!("No forge is configured; there are no merge-hold labels to provision.");
    }
}
