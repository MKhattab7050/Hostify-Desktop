#[cfg(windows)]fn main(){if let Err(e)=hostify_engine::cli::main_entry(){eprintln!("{e}");std::process::exit(1)}}
#[cfg(not(windows))]fn main(){eprintln!("Hostify engine requires Windows");std::process::exit(1)}
