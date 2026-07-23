use std::{env, fs, path::PathBuf};

fn main() -> anyhow::Result<()> {
    let mut args = env::args().skip(1);
    let output_path = args.next().map(PathBuf::from);
    let document = desk_foreman::api::openapi_document();
    let json = serde_json::to_string_pretty(&document)?;

    if let Some(path) = output_path {
        fs::write(path, json)?;
    } else {
        println!("{json}");
    }

    Ok(())
}
