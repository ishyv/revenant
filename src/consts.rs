
/// Output directory to place the generated app project.
pub const DEFAULT_ROOT: &str = "./output";

pub const REQUIRED_PACKAGES: &[&str] = &[
    "git", 
    "node", 
    "npm",
    
    #[cfg(windows)]
    "choco"
    ];

pub const NPM_DEFAULTS: &str = "defaults.json";

/// Contents of the default Svelte project to copy over.
/// This provides a basic setup of the necessary files and folders for the app to run.
pub const DEFAULT_SVELTE_PROJECT: &str = "default-svelte-root";