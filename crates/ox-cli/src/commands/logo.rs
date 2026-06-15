use anyhow::Result;
use console::Term;

const LOGO: &str = "  ___             __  __       _
 / _ \\__  ___   _|  \\/  | __ _| | _____
| | | \\ \\/ / | | | |\\/| |/ _` | |/ / _ \\
| |_| |>  <| |_| | |  | | (_| |   <  __/
 \\___//_/\\_\\\\__, |_|  |_|\\__,_|_|\\_\\___|
            |___/";

pub fn cmd_logo() -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    let term = Term::stderr();
    let use_color = term.features().colors_supported() && std::env::var("NO_COLOR").is_err();

    if use_color {
        println!("\x1b[36m{LOGO}\x1b[0m");
        println!("\n  \x1b[1mOxyMake v{version}\x1b[0m — workflow orchestration");
    } else {
        println!("{LOGO}");
        println!("\n  OxyMake v{version} — workflow orchestration");
    }
    Ok(())
}
