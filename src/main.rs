// Copyright 2025 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use clap::{Parser, Subcommand};
use operator::{crd, run};

#[derive(Parser)]
#[command(name = "rustfs-op")]
#[command(about = "RustFS Kubernetes Operator CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Output CRDs in YAML
    Crd {
        /// Optional output path. If not set, the output will be written to stdout.
        #[arg(short, long)]
        file: Option<String>,
    },

    /// Run the controller
    Server {},
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Crd { file } => crd(file).await,
        Commands::Server {} => run().await,
    }
}
