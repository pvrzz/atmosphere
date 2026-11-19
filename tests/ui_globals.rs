// atmosphere · ui_globals.rs · Fails if two UI scripts declare the same global name, since a later script silently replaces the earlier one
use std::collections::HashMap;

#[test]
fn ui_scripts_share_no_global_names() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut clashes = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let line = line.strip_prefix("async ").unwrap_or(line);
            let rest = ["function ", "const ", "let ", "class "].iter().find_map(|k| line.strip_prefix(k));
            let Some(rest) = rest else { continue };
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
            if name.is_empty() {
                continue;
            }
            if let Some(prev) = seen.insert(name.clone(), file.clone()) {
                clashes.push(format!("{name} in {prev} and {file}"));
            }
        }
    }
    assert!(clashes.is_empty(), "duplicate globals: {clashes:?}");
}
