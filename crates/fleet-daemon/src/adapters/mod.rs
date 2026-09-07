//! Boundary traits and concrete adapters for operating-system and external-tool access.

use std::sync::Arc;

pub mod clock;
pub mod files;
pub mod git;
pub mod github;
pub mod logs;
pub mod process;
pub mod shell;

use files::Files;
use git::{Git, ShellGit};
use github::{GhCli, Github};
use process::{Process, RealProcess};
use shell::{RealShell, Shell};

/// Cloneable dependency bundle shared by daemon domain services.
#[derive(Clone)]
pub struct Adapters {
    /// Git command boundary.
    pub git: Arc<dyn Git>,
    /// GitHub metadata boundary.
    pub github: Arc<dyn Github>,
    /// Process and listening-port observation boundary.
    pub process: Arc<dyn Process>,
    /// Filesystem operation boundary.
    pub files: Arc<dyn Files>,
    /// General external-command boundary.
    pub shell: Arc<dyn Shell>,
}

impl Adapters {
    /// Creates the production command adapters around a configured filesystem boundary.
    #[must_use]
    pub fn system(files: Arc<dyn Files>) -> Self {
        let shell: Arc<dyn Shell> = Arc::new(RealShell);
        Self {
            git: Arc::new(ShellGit::new(Arc::clone(&shell))),
            github: Arc::new(GhCli::new(Arc::clone(&shell))),
            process: Arc::new(RealProcess::new(Arc::clone(&shell))),
            files,
            shell,
        }
    }
}
