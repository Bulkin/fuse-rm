use std::io;

mod rmxfs;

use rmxfs::RMXFS;

#[derive(Debug)]
struct ProgError(String);

impl std::convert::From<argwerk::Error> for ProgError {
    fn from(err: argwerk::Error) -> ProgError {
        match err.kind() {
            argwerk::ErrorKind::UnsupportedArgument{argument} => ProgError(
                format!("Unsupported argument: {}", argument)),
            argwerk::ErrorKind::UnsupportedSwitch{switch} => ProgError(
                format!("Unsupported switch: {}", switch)),
            argwerk::ErrorKind::MissingSwitchArgument{switch, argument} => ProgError(
                format!("Missing arg {} for switch {}", argument, switch)),
            argwerk::ErrorKind::MissingPositional{name} => ProgError(
                format!("Missing positional arg {}", name)),
            argwerk::ErrorKind::MissingRequired{name, ..} => ProgError(
                format!("Missing required arg {}", name)),
            argwerk::ErrorKind::InputError{error} => ProgError(
                format!("Input error {}", error)),
            argwerk::ErrorKind::Error{name, ..} => ProgError(
                format!("Error: {}", name)),
        }
    }
}           
        

impl std::convert::From<io::Error> for ProgError {
    fn from(err: io::Error) -> ProgError {
        ProgError(format!("IO error: {}", err))
    }
}

fn main() -> Result<(), ProgError> {
    let args = argwerk::args! {
        /// A FUSE fs for accessing xochitl data.
        "fuse-rm [opts] source target" {
            help: bool,
            help_txt: String,
            limit: usize = 10,
            positional: Option<(String, String)>,
        }
        /// The limit of the operation. (default: 10).
        ["-l" | "--limit", int] => {
            limit = str::parse(&int)?;
        }
        /// Print this help.
        ["-h" | "--help"] => {
            println!("{}", HELP);
            help = true;
        }
        /// <source> and <target> paths for mounting
        [source, target] if positional.is_none() => {
            positional = Some((source, target))
        }
    }?;

    if args.help {
        return Ok(());
    }

    if args.positional.is_none() {
        println!("Source and target paths required");
        return Err(ProgError(format!("Missing positional args")));
    }

    let (source_dir, target_dir) = &args.positional.unwrap();
    
    fuser::mount2(RMXFS::new(source_dir), target_dir, &[])?;
    
    Ok(())
}
