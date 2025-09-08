pub const DEFAULT_ROOT: &str = "./output";

pub const REQUIRED_PACKAGES: &[&str] = &[
    "git", 
    "node", 
    "npm",
    
    #[cfg(windows)]
    "choco"
    ];
