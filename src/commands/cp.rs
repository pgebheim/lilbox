use anyhow::{Result, bail};

use crate::app::App;
use crate::sandbox::connect_box;

pub(crate) fn box_path(value: &str) -> Option<(&str, &str)> {
    let (name, path) = value.split_once(':')?;
    (!name.is_empty() && path.starts_with('/')).then_some((name, path))
}

pub(crate) async fn cp(app: &App, src: String, dst: String) -> Result<()> {
    match (box_path(&src), box_path(&dst)) {
        (Some((name, guest)), None) => {
            let sandbox = connect_box(app, name, true).await?;
            sandbox.fs().copy_to_host(guest, &dst).await?;
        }
        (None, Some((name, guest))) => {
            let sandbox = connect_box(app, name, true).await?;
            sandbox.fs().copy_from_host(&src, guest).await?;
        }
        _ => bail!("exactly one side must be a box path in NAME:/absolute/path form"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_box_copy_paths() {
        assert_eq!(box_path("web:/root/out"), Some(("web", "/root/out")));
        assert_eq!(box_path("./local:file"), None);
        assert_eq!(box_path("web:relative"), None);
    }
}
