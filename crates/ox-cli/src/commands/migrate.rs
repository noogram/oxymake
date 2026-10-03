//! Review or apply a fully resolved include-graph migration.
use anyhow::Result;
use std::path::Path;

#[derive(clap::Args)]
pub struct MigrateArgs {
    /// Oxymakefile path (includes are resolved relative to each file)
    #[arg(short = 'f', long, default_value = "Oxymakefile.toml")]
    pub file: String,
    /// Destination schema version
    #[arg(long, value_parser = ["2"])]
    pub to_format: String,
    /// Apply the reviewed migration to every file in the include graph
    #[arg(long)]
    pub write: bool,
}

pub fn cmd_migrate(args: MigrateArgs) -> Result<()> {
    let migration = ox_format::migrate::prepare(Path::new(&args.file))?;
    print!("{}", migration.report());
    if args.write {
        migration.write()?;
        println!("Migration applied to the include graph.");
    }
    Ok(())
}
