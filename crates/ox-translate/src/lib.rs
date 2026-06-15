//! # ox-translate — Workflow Format Translator
//!
//! This crate translates workflow files between OxyMake, Snakemake, and WDL formats.
//! It provides:
//!
//! - A Snakemake parser that produces an intermediate representation (IR)
//! - A WDL parser that produces the same IR
//! - A TOML generator that converts IR into `Oxymakefile.toml`
//! - A Snakemake generator that converts IR into Snakefiles
//! - A WDL generator that converts IR into WDL files
//! - Structured escalation tracking for constructs requiring manual review
//!
//! ## Quick start (Snakemake)
//!
//! ```
//! use ox_translate::snakemake::parser::parse_snakefile;
//! use ox_translate::oxymake::generator::generate_oxymakefile;
//!
//! let snakefile = r#"
//! rule process:
//!     input:
//!         "data/{sample}.csv"
//!     output:
//!         "results/{sample}.txt"
//!     shell:
//!         "sort {input} > {output}"
//! "#;
//!
//! let ir = parse_snakefile(snakefile).unwrap();
//! let toml = generate_oxymakefile(&ir);
//! assert!(toml.contains("[rule.process]"));
//! ```
//!
//! ## Quick start (WDL)
//!
//! ```
//! use ox_translate::wdl::parser::parse_wdl;
//! use ox_translate::oxymake::generator::generate_oxymakefile;
//!
//! let wdl = r#"
//! version 1.0
//!
//! task process {
//!     input {
//!         File input_file
//!     }
//!     command {
//!         sort ~{input_file} > output.txt
//!     }
//!     output {
//!         File result = "output.txt"
//!     }
//! }
//! "#;
//!
//! let ir = parse_wdl(wdl).unwrap();
//! let toml = generate_oxymakefile(&ir);
//! assert!(toml.contains("[rule.process]"));
//! ```

pub mod export;
pub mod ir;
pub mod oxymake;
pub mod snakemake;
pub mod wdl;
