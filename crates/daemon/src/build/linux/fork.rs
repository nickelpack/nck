#![tarpaulin::skip]

// https://github.com/containers/youki/blob/main/crates/libcontainer/src/process/fork.rs

use std::{ffi::c_int, fs::File, num::NonZeroUsize};

use libc::SIGCHLD;
use nix::{
    libc,
    sched::CloneFlags,
    sys::{mman, resource},
    unistd::Pid,
};

#[derive(Debug, thiserror::Error)]
pub enum CloneError {
    #[error("failed to clone process")]
    Clone(#[source] nix::Error),
    #[error("failed to get system memory page size")]
    PageSize(#[source] nix::Error),
    #[error("failed to get resource limit")]
    ResourceLimit(#[source] nix::Error),
    #[error("the stack size is zero")]
    ZeroStackSize,
    #[error("failed to allocate stack")]
    StackAllocation(#[source] nix::Error),
    #[error("failed to create stack guard page")]
    GuardPage(#[source] nix::Error),
    #[error("unknown error code {0}")]
    UnknownErrno(i32),
}

#[derive(Debug, thiserror::Error)]
pub enum ForkError {
    #[error("failed to fork process")]
    Fork(#[source] nix::Error),
    #[error("the child returned with {0:x}")]
    ErrorCode(i32),
}

/// The callback function used in clone system call. The return value is i32
/// which is consistent with C functions return code. The trait has to be
/// `FnMut` because we need to be able to call the closure multiple times, once
/// in clone3 and once in clone if fallback is required. The closure is boxed
/// because we need to store the closure on heap, not stack in the case of
/// `clone`. Unlike `fork` or `clone3`, the `clone` glibc wrapper requires us to
/// pass in a child stack, which is empty. By storing the closure in heap, we
/// can then in the new process to re-box the heap memory back to a closure
/// correctly.
pub type CloneCb<R> = Box<dyn FnMut() -> R>;

/// A type that can be converted into an exit code.
pub trait IntoExitCode {
    /// Converts the current type into an exit code.
    fn report(&self) -> i32;
}

impl<T, E: IntoExitCode> IntoExitCode for Result<T, E> {
    fn report(&self) -> i32 {
        match self {
            Ok(_) => 0,
            Err(v) => v.report(),
        }
    }
}

impl IntoExitCode for anyhow::Error {
    fn report(&self) -> i32 {
        tracing::error!(?self, "process failed");
        -1
    }
}

impl IntoExitCode for i32 {
    fn report(&self) -> i32 {
        *self
    }
}

// Clone a child process and execute the callback.
pub fn clone<R: IntoExitCode + std::fmt::Debug>(
    cb: CloneCb<R>,
    clone_flags: CloneFlags,
) -> Result<Pid, CloneError> {
    if clone_flags.intersects(CloneFlags::CLONE_PARENT) {
        clone_internal(cb, clone_flags.bits() as u64, None)
    } else {
        clone_internal(cb, clone_flags.bits() as u64, Some(SIGCHLD as u64))
    }
}

// An internal wrapper to manage the clone3 vs clone fallback logic.
fn clone_internal<R: IntoExitCode + std::fmt::Debug>(
    mut cb: CloneCb<R>,
    flags: u64,
    exit_signal: Option<u64>,
) -> Result<Pid, CloneError> {
    match clone3(&mut cb, flags, exit_signal) {
        Ok(pid) => Ok(pid),
        // For now, we decide to only fallback on ENOSYS
        Err(CloneError::Clone(nix::Error::ENOSYS)) => {
            tracing::debug!("clone3 is not supported, fallback to clone");
            let pid = clone_fallback(cb, flags, exit_signal)?;

            Ok(pid)
        }
        Err(err) => Err(err),
    }
}

// Unlike the clone call, clone3 is currently using the kernel syscall, mimicking
// the interface of fork. There is not need to explicitly manage the memory, so
// we can safely passing the callback closure as reference.
fn clone3<R: IntoExitCode + std::fmt::Debug>(
    cb: &mut CloneCb<R>,
    flags: u64,
    exit_signal: Option<u64>,
) -> Result<Pid, CloneError> {
    #[repr(C)]
    struct Clone3Args {
        flags: u64,
        pidfd: u64,
        child_tid: u64,
        parent_tid: u64,
        exit_signal: u64,
        stack: u64,
        stack_size: u64,
        tls: u64,
        set_tid: u64,
        set_tid_size: u64,
        cgroup: u64,
    }
    let mut args = Clone3Args {
        flags,
        pidfd: 0,
        child_tid: 0,
        parent_tid: 0,
        exit_signal: exit_signal.unwrap_or(0),
        stack: 0,
        stack_size: 0,
        tls: 0,
        set_tid: 0,
        set_tid_size: 0,
        cgroup: 0,
    };
    let args_ptr = &mut args as *mut Clone3Args;
    let args_size = std::mem::size_of::<Clone3Args>();
    // For now, we can only use clone3 as a kernel syscall. Libc wrapper is not
    // available yet. This can have undefined behavior because libc authors do
    // not like people calling kernel syscall to directly create processes. Libc
    // does perform additional bookkeeping when calling clone or fork. So far,
    // we have not observed any issues with calling clone3 directly, but we
    // should keep an eye on it.
    match unsafe { libc::syscall(libc::SYS_clone3, args_ptr, args_size) } {
        -1 => Err(CloneError::Clone(nix::Error::last())),
        0 => {
            // Inside the cloned process, we execute the callback and exit with
            // the return code.
            std::process::exit(cb().report());
        }
        ret if ret >= 0 => Ok(Pid::from_raw(ret as i32)),
        ret => Err(CloneError::UnknownErrno(ret as i32)),
    }
}

fn clone_fallback<R: IntoExitCode + std::fmt::Debug>(
    cb: CloneCb<R>,
    flags: u64,
    exit_signal: Option<u64>,
) -> Result<Pid, CloneError> {
    const DEFAULT_STACK_SIZE: usize = 8 * 1024 * 1024; // 8M
    const DEFAULT_PAGE_SIZE: usize = 4 * 1024; // 4K

    // Use sysconf to find the page size. If there is an error, we assume
    // the default 4K page size.
    let page_size = nix::unistd::sysconf(nix::unistd::SysconfVar::PAGE_SIZE)
        .map_err(CloneError::PageSize)?
        .map(|size| size as usize)
        .unwrap_or(DEFAULT_PAGE_SIZE);

    // Find out the default stack max size through getrlimit.
    let (rlim_cur, _) =
        resource::getrlimit(resource::Resource::RLIMIT_STACK).map_err(CloneError::ResourceLimit)?;
    // mmap will return ENOMEM if stack size is unlimited when we create the
    // child stack, so we need to set a reasonable default stack size.
    let default_stack_size = if rlim_cur != u64::MAX {
        rlim_cur as usize
    } else {
        tracing::debug!(
            "stack size returned by getrlimit() is unlimited, use DEFAULT_STACK_SIZE(8MB)"
        );
        DEFAULT_STACK_SIZE
    };

    // Using the clone syscall requires us to create the stack space for the
    // child process instead of taken cared for us like fork call. We use mmap
    // here to create the stack.  Instead of guessing how much space the child
    // process needs, we allocate through mmap to the system default limit,
    // which is 8MB on most of the linux system today. This is OK since mmap
    // will only reserve the address space upfront, instead of allocating
    // physical memory upfront.  The stack will grow as needed, up to the size
    // reserved, so no wasted memory here. Lastly, the child stack only needs
    // to support the container init process set up code in Youki. When Youki
    // calls exec into the container payload, exec will reset the stack.  Note,
    // do not use MAP_GROWSDOWN since it is not well supported.
    // Ref: https://man7.org/linux/man-pages/man2/mmap.2.html
    let child_stack = unsafe {
        // Since nix = "0.27.1", `mmap()` requires a generic type `F: AsFd`.
        // `::<File>` doesn't have any meaning because we won't use it.
        mman::mmap::<File>(
            None,
            NonZeroUsize::new(default_stack_size).ok_or(CloneError::ZeroStackSize)?,
            mman::ProtFlags::PROT_READ | mman::ProtFlags::PROT_WRITE,
            mman::MapFlags::MAP_PRIVATE | mman::MapFlags::MAP_ANONYMOUS | mman::MapFlags::MAP_STACK,
            None,
            0,
        )
        .map_err(CloneError::StackAllocation)?
    };
    unsafe {
        // Consistent with how pthread_create sets up the stack, we create a
        // guard page of 1 page, to protect the child stack collision. Note, for
        // clone call, the child stack will grow downward, so the bottom of the
        // child stack is in the beginning.
        mman::mprotect(child_stack, page_size, mman::ProtFlags::PROT_NONE)
            .map_err(CloneError::GuardPage)?;
    };

    // Since the child stack for clone grows downward, we need to pass in
    // the top of the stack address.
    let child_stack_top = unsafe { child_stack.add(default_stack_size) };

    // Combine the clone flags with exit signals.
    let combined_flags = (flags | exit_signal.unwrap_or(0)) as c_int;

    // We are passing the boxed closure "cb" into the clone function as the a
    // function pointer in C. The box closure in Rust is both a function pointer
    // and a struct. However, when casting the box closure into libc::c_void,
    // the function pointer will be lost. Therefore, to work around the issue,
    // we double box the closure. This is consistent with how std::unix::thread
    // handles the closure.
    // Ref: https://github.com/rust-lang/rust/blob/master/library/std/src/sys/unix/thread.rs
    let data = Box::into_raw(Box::new(cb));
    // The main is a wrapper function passed into clone call below. The "data"
    // arg is actually a raw pointer to the Box closure. so here, we re-box the
    // pointer back into a box closure so the main takes ownership of the
    // memory. Then we can call the closure.
    extern "C" fn main<RR: IntoExitCode + std::fmt::Debug>(data: *mut libc::c_void) -> libc::c_int {
        unsafe { Box::from_raw(data as *mut CloneCb<RR>)().report() }
    }

    // The nix::sched::clone wrapper doesn't provide the right interface.  Using
    // the clone syscall is one of the rare cases where we don't want rust to
    // manage the child stack memory. Instead, we want to use c_void directly
    // here.  Therefore, here we are using libc::clone syscall directly for
    // better control.  The child stack will be cleaned when exec is called or
    // the child process terminates. The nix wrapper also does not treat the
    // closure memory correctly. The wrapper implementation fails to pass the
    // right ownership to the new child process.
    // Ref: https://github.com/nix-rust/nix/issues/919
    // Ref: https://github.com/nix-rust/nix/pull/920
    let ret = unsafe {
        libc::clone(
            main::<R>,
            child_stack_top,
            combined_flags,
            data as *mut libc::c_void,
        )
    };

    // After the clone returns, the heap memory associated with the Box closure
    // is duplicated in the cloned process. Therefore, we can safely re-box the
    // closure from the raw pointer and let rust to continue managing the
    // memory. We call drop here explicitly to avoid the warning that the
    // closure is not used. This is correct since the closure is called in the
    // cloned process, not the parent process.
    unsafe { drop(Box::from_raw(data)) };
    match ret {
        -1 => Err(CloneError::Clone(nix::Error::last())),
        pid if ret > 0 => Ok(Pid::from_raw(pid)),
        _ => unreachable!("clone returned a negative pid {ret}"),
    }
}

#[cfg(all(test, not(tarpaulin)))]
mod test {
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
    };

    use super::*;
    use anyhow::{bail, Result};
    use nix::{
        sys::wait::{waitpid, WaitStatus},
        unistd::ForkResult,
    };

    #[test]
    fn test_clone() -> Result<()> {
        let pid = clone(Box::new(|| 0), CloneFlags::empty())?;
        match waitpid(pid, None).expect("wait pid failed.") {
            WaitStatus::Exited(p, status) => {
                assert_eq!(pid, p);
                assert_eq!(status, 0);
                Ok(())
            }
            _ => bail!("test failed"),
        }
    }

    #[test]
    fn test_clone_err() -> Result<()> {
        let pid = clone(Box::new(|| -1), CloneFlags::empty())?;
        match waitpid(pid, None).expect("wait pid failed.") {
            WaitStatus::Exited(p, status) => {
                assert_eq!(pid, p);
                assert_eq!(status, 255);
                Ok(())
            }
            _ => bail!("test failed"),
        }
    }

    #[test]
    fn test_clone_parent() -> Result<()> {
        // The `container_clone_sibling` will create a sibling process (share
        // the same parent) of the calling process. In Unix, a process can only
        // wait on the immediate children process and can't wait on the sibling
        // process. Therefore, to test the logic, we will have to fork a process
        // first and then let the forked process call `container_clone_sibling`.
        // Then the testing process (the process where test is called), who are
        // the parent to this forked process and the sibling process cloned by
        // the `container_clone_sibling`, can wait on both processes.

        // We need to use a channel so that the forked process can pass the pid
        // of the sibling process to the testing process.
        let (mut child_socket, mut server_socket) = UnixStream::pair()?;

        match unsafe { nix::unistd::fork() }? {
            ForkResult::Parent { child } => {
                let mut sibling_process_pid = [0u8; std::mem::size_of::<i32>()];
                server_socket.read_exact(&mut sibling_process_pid)?;
                let sibling_process_pid = i32::from_ne_bytes(sibling_process_pid);
                let sibling_process_pid = Pid::from_raw(sibling_process_pid);
                match waitpid(sibling_process_pid, None).expect("wait pid failed.") {
                    WaitStatus::Exited(p, status) => {
                        assert_eq!(sibling_process_pid, p);
                        assert_eq!(status, 0);
                    }
                    _ => bail!("failed to wait on the sibling process"),
                }
                // After sibling process exits, we can wait on the forked process.
                match waitpid(child, None).expect("wait pid failed.") {
                    WaitStatus::Exited(p, status) => {
                        assert_eq!(child, p);
                        assert_eq!(status, 0);
                    }
                    _ => bail!("failed to wait on the forked process"),
                }
            }
            ForkResult::Child => {
                // Inside the forked process. We call `container_clone` and pass
                // the pid to the parent process.
                let pid = clone(Box::new(|| 0), CloneFlags::CLONE_PARENT)?;
                let mut pid = pid.as_raw().to_ne_bytes();
                child_socket.write_all(&mut pid)?;
                std::process::exit(0);
            }
        };

        Ok(())
    }
}
