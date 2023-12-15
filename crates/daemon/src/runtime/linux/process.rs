use super::{channel, fork, user_ns};

mod main_process;
mod zygote_process;

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error(transparent)]
    Channel(#[from] channel::ChannelError),
    #[error("failed to write deny to setgroups")]
    SetGroupsDeny(#[source] std::io::Error),
    #[error(transparent)]
    UserNamespace(#[from] user_ns::UserNamespaceError),
    #[error("container state is required")]
    ContainerStateRequired,
    #[error("failed to wait for intermediate process")]
    WaitIntermediateProcess(#[source] nix::Error),
    #[error("failed to create intermediate process")]
    IntermediateProcessFailed(#[source] fork::CloneError),
}
