use std::{env, fs, path::PathBuf};

fn main() -> anyhow::Result<()> {
    let mut args = env::args_os().skip(1);
    let path = args.next().map(PathBuf::from);
    if args.next().is_some() {
        anyhow::bail!("expected at most one output path");
    }
    let document = serde_json::json!({
        "schema_version": 1,
        "tools": desk_foreman::tools::DeskForemanService::tool_catalog(),
    });
    let json = format!("{}\n", serde_json::to_string_pretty(&document)?);
    if let Some(path) = path {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, json)?;
    } else {
        print!("{json}");
    }
    Ok(())
}
