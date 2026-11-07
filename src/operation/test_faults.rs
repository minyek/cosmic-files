use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CHILD_DIRECTORY: &str = "COSMIC_OPERATION_FAULT_TEST_DIRECTORY";
#[cfg(target_os = "linux")]
const RENAME_ERRNO: &str = "COSMIC_OPERATION_FAULT_TEST_RENAME_ERRNO";

pub(super) enum Fault {
    FileSizeLimit,
    #[cfg(target_os = "linux")]
    RenameError(i32),
}

pub(super) fn child_directory() -> Option<PathBuf> {
    std::env::var_os(CHILD_DIRECTORY).map(PathBuf::from)
}

pub(super) fn run_child(test_name: &str, directory: &Path, fault: Fault) -> io::Result<()> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            &format!("operation::tests::{test_name}"),
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_DIRECTORY, directory);
    #[cfg(target_os = "linux")]
    if let Fault::RenameError(errno) = &fault {
        command.env(RENAME_ERRNO, errno.to_string());
    }
    // Faults apply before exec so all of compio's workers inherit them.
    unsafe {
        command.pre_exec(move || match fault {
            Fault::FileSizeLimit => {
                if libc::signal(libc::SIGXFSZ, libc::SIG_IGN) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
                if libc::setrlimit(
                    libc::RLIMIT_FSIZE,
                    &libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    },
                ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            Fault::RenameError(errno) => install_rename_fault(errno),
        });
    }
    let output = command.output()?;
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
        "fault test child did not execute {test_name}: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.status.success(),
        "fault test child failed: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(target_os = "linux")]
unsafe fn install_rename_fault(errno: i32) -> io::Result<()> {
    let filters = [
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: libc::SYS_renameat2 as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | errno as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ];
    let program = libc::sock_fprog {
        len: filters.len() as u16,
        filter: filters.as_ptr().cast_mut(),
    };
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn assert_rename_fault() {
    let errno: i32 = std::env::var(RENAME_ERRNO)
        .expect("rename fault errno missing")
        .parse()
        .expect("invalid rename fault errno");
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            std::ptr::null::<libc::c_char>(),
            libc::AT_FDCWD,
            std::ptr::null::<libc::c_char>(),
            libc::RENAME_NOREPLACE,
        )
    };
    assert_eq!(result, -1, "rename fault was not installed");
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(errno));
}
