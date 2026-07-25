//! Cross-platform process-group creation and descendant termination.

use std::io;
use std::process::{ChildStderr, ChildStdout, Command, ExitStatus};

use command_group::{CommandGroup, GroupChild};

pub(crate) fn spawn(command: &mut Command) -> io::Result<GroupChild> {
    command.group_spawn()
}

pub(crate) fn take_stdout(child: &mut GroupChild) -> Option<ChildStdout> {
    child.inner().stdout.take()
}

pub(crate) fn take_stderr(child: &mut GroupChild) -> Option<ChildStderr> {
    child.inner().stderr.take()
}

pub(crate) fn terminate(child: &mut GroupChild) -> io::Result<bool> {
    match child.kill() {
        Ok(()) => Ok(true),
        Err(error) if process_is_gone(&error) => Ok(true),
        Err(error) => Err(error),
    }
}

pub(crate) fn wait(child: &mut GroupChild) -> io::Result<ExitStatus> {
    child.wait()
}

pub(crate) fn try_wait(child: &mut GroupChild) -> io::Result<Option<ExitStatus>> {
    child.try_wait()
}

fn process_is_gone(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::InvalidInput | io::ErrorKind::NotFound
    ) || cfg!(unix) && error.raw_os_error() == Some(3)
}
