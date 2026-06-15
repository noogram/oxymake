//! Implementation of the `ox init` command.

use std::path::Path;

use anyhow::{Context, Result, bail};

#[derive(clap::Args)]
pub struct InitArgs {
    /// Directory to initialize (defaults to current directory)
    #[arg(default_value = ".")]
    pub dir: String,

    /// Overwrite existing Oxymakefile
    #[arg(long)]
    pub force: bool,
}

const STARTER_OXYMAKEFILE: &str = r#"ox_version = "0.1"
format_version = "1"

[config]
samples = ["A", "B", "C"]

# The default target: build all results.
[rule.all]
input = ["results/{sample}.txt"]

# Process each sample: sort the input lines and write to output.
[rule.process]
input = ["data/{sample}.txt"]
output = ["results/{sample}.txt"]
shell = "mkdir -p results && sort {input} > {output}"
"#;

pub fn cmd_init(args: InitArgs) -> Result<()> {
    let dir = Path::new(&args.dir);

    // Create the directory if it doesn't exist.
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("cannot create directory: {}", dir.display()))?;
    }

    let oxymakefile = dir.join("Oxymakefile.toml");
    if oxymakefile.exists() && !args.force {
        bail!(
            "Oxymakefile.toml already exists in {}. Use --force to overwrite.",
            dir.display()
        );
    }

    std::fs::write(&oxymakefile, STARTER_OXYMAKEFILE)
        .with_context(|| format!("cannot write {}", oxymakefile.display()))?;

    // Create the .oxymake directory for state/cache.
    let oxymake_dir = dir.join(".oxymake");
    if !oxymake_dir.exists() {
        std::fs::create_dir_all(&oxymake_dir)
            .with_context(|| format!("cannot create {}", oxymake_dir.display()))?;
    }

    println!("Initialized OxyMake project in {}", dir.display());
    println!("  Created: Oxymakefile.toml");
    println!("  Created: .oxymake/");

    Ok(())
}
