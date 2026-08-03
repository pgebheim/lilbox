use anyhow::{Result, bail};

use crate::model::Template;

pub(crate) fn builtin_template(name: &str) -> Option<Template> {
    let (manifest, setup) = match name {
        "python-dev" => (
            include_str!("../templates/python-dev/template.json"),
            include_str!("../templates/python-dev/setup.sh"),
        ),
        "node-dev" => (
            include_str!("../templates/node-dev/template.json"),
            include_str!("../templates/node-dev/setup.sh"),
        ),
        _ => return None,
    };
    Some(Template {
        name: name.into(),
        source: "builtin",
        dir: None,
        manifest: serde_json::from_str(manifest).expect("valid built-in template"),
        setup: Some(setup.into()),
        dockerfile: false,
    })
}

pub(crate) fn validate_template_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        bail!("invalid template name '{name}'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_embedded() {
        let python = builtin_template("python-dev").unwrap();
        assert_eq!(python.manifest.image.as_deref(), Some("python"));
        assert!(python.setup.unwrap().contains("uv"));
    }

    #[test]
    fn rejects_template_traversal() {
        assert!(validate_template_name("../secret").is_err());
        assert!(validate_template_name("normal-name").is_ok());
    }
}
