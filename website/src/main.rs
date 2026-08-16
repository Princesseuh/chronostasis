mod components;
mod layout;

use maudit::{AssetsOptions, BuildOptions, BuildOutput, content_sources, coronate, routes};

mod routes {
    mod cli;
    mod download;
    mod getting_started;
    mod index;
    mod model_swap;
    mod troubleshooting;
    pub use cli::CliReference;
    pub use download::DownloadTarget;
    pub use getting_started::GettingStarted;
    pub use index::Index;
    pub use model_swap::ModelSwap;
    pub use troubleshooting::Troubleshooting;
}

use routes::{CliReference, DownloadTarget, GettingStarted, Index, ModelSwap, Troubleshooting};

fn main() -> Result<BuildOutput, Box<dyn std::error::Error>> {
    coronate(
        routes![
            Index,
            DownloadTarget,
            GettingStarted,
            CliReference,
            ModelSwap,
            Troubleshooting
        ],
        content_sources![],
        BuildOptions {
            assets: AssetsOptions {
                tailwind_binary_path: "./node_modules/.bin/tailwindcss".into(),
                ..Default::default()
            },
            ..Default::default()
        },
    )
}
