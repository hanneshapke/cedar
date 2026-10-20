//! The community tool registry in `registry/`: cedar itself accepts every tool in it, and the
//! index lists each with the permissions its manifest asks for and the hashes of its files, which
//! is what an install checks (see `src/marketplace.rs`).

use cedar::marketplace::{check_file_name, Index};
use cedar::tools::custom::{load_folder, PythonTool};
use cedar::tools::{Tool, ToolSet};
use sha2::{Digest, Sha256};
use std::path::Path;

#[test]
fn every_registry_tool_is_one_cedar_would_install() {
    let root = Path::new("registry");
    let index: Index =
        serde_json::from_str(&std::fs::read_to_string(root.join("index.json")).unwrap()).unwrap();
    assert!(!index.tools.is_empty());
    let builtin = ToolSet::standard();
    for listing in &index.tools {
        let folder = root.join("tools").join(&listing.name);
        let tool: PythonTool = load_folder(&folder, &listing.name)
            .unwrap_or_else(|e| panic!("{}: {e:#}", listing.name))
            .unwrap_or_else(|| panic!("{}: no tool.json", listing.name));
        assert!(
            builtin.get(tool.name()).is_none(),
            "{} is a built-in name",
            listing.name
        );
        assert_eq!(
            tool.version(),
            Some(listing.version.as_str()),
            "{}",
            listing.name
        );
        assert_eq!(tool.permissions(), listing.permissions, "{}", listing.name);
        assert!(
            tool.installed().is_none(),
            "{} ships an install receipt",
            listing.name
        );
        assert_eq!(listing.base_url, format!("tools/{}/", listing.name));
        for (file, sha) in &listing.files {
            check_file_name(file).unwrap();
            let body = std::fs::read(folder.join(file)).unwrap();
            assert_eq!(
                &hex::encode(Sha256::digest(&body)),
                sha,
                "{}/{file}",
                listing.name
            );
        }
    }
}
