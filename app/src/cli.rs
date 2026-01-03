//! common CLI args for all bins

use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug, Clone)]
pub struct VmiArgs {
    #[arg(short, long)]
    pub name: String,
    #[arg(short, long)]
    pub json: PathBuf,
    #[arg(short = 'k', long, default_value = "/tmp/introspector")]
    pub socket_path: PathBuf,
}
