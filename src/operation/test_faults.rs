use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CHILD_DIRECTORY: &str = "COSMIC_OPERATION_FAULT_TEST_DIRECTORY";

pub(super) enum Fault {
    FileSizeLimit,
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
