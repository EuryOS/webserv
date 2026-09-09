use std::fs;
use std::path::PathBuf;

fn rust_identifier(name: &str) -> String {
    let mut out = String::new();
    for (index, byte) in name.bytes().enumerate() {
        if (byte.is_ascii_alphanumeric() || byte == b'_') && !(index == 0 && byte.is_ascii_digit())
        {
            out.push(byte as char);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "grant".to_string()
    } else {
        out
    }
}

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("package")
        .join("package.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let text = fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("read {}: {error}", manifest.display()));
    let mut in_webserv_capabilities = false;
    let mut names = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_webserv_capabilities = trimmed == "[component.webserv.capabilities]";
            continue;
        }
        if in_webserv_capabilities {
            if let Some((role, _)) = trimmed.split_once('=') {
                let role = role.trim();
                if !role.is_empty() {
                    names.push((role.to_string(), rust_identifier(role)));
                }
            }
        }
    }
    if names.is_empty() {
        panic!("{} declares no webserv capabilities", manifest.display());
    }

    let mut generated =
        String::from("pub struct Grants<'a> { directory: eury_sdk::grants::Directory<'a> }\n\n");
    generated.push_str("impl<'a> Grants<'a> {\n");
    generated.push_str(
        "    pub fn from_directory(directory: eury_sdk::grants::Directory<'a>) -> Self {\n\
             Self { directory }\n\
         }\n\n",
    );
    generated.push_str(
        "    pub fn from_startup() -> Option<Grants<'static>> {\n\
             eury_sdk::grants::Directory::from_startup().map(Grants::from_directory)\n\
         }\n\n",
    );
    for (name, identifier) in names {
        generated.push_str(&format!(
            "    pub fn {identifier}(&self) -> Option<eury_sdk::grants::Grant<'_>> {{\n\
                 self.directory.lookup({name:?})\n\
             }}\n\n"
        ));
    }
    generated.push_str("}\n");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    fs::write(out.join("grants.rs"), generated).expect("write generated grants.rs");
}
