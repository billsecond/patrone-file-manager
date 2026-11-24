//! The computer, from the Flatpak. Installed as a package, File Manager
//! runs on the computer itself, and every function here gives back what it
//! was given. In the Flatpak (from Patrone's app index):
//!
//! - The computer's own files are at their own paths (--filesystem=host),
//!   except its system folders (/usr, /etc, /bin, /sbin, /lib*), whose
//!   places the runtime's take; the sandbox has the computer's under
//!   /run/host (host-os, host-etc). `inside` maps a path to where it can be
//!   read, `outside` maps it back to the path the computer knows, which is
//!   the one shown and the one given to its programs. Tabs show a folder
//!   by its inside path (Tab::show), so its listing, its files and their
//!   children are the computer's.
//! - Programs are the computer's: `command` and `argv` run them with
//!   `flatpak-spawn --host`, in the folder given, with every /run/host path
//!   among their arguments put back as the computer's.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use gtk::{gio, glib, prelude::*};

/// The computer's system folders, which the sandbox has under /run/host.
const SYSTEM: [&str; 8] = [
    "usr", "etc", "bin", "sbin", "lib", "lib32", "lib64", "libx32",
];
const HOST: &str = "/run/host";

/// Whether this is the Flatpak.
pub fn sandboxed() -> bool {
    static SANDBOXED: OnceLock<bool> = OnceLock::new();
    *SANDBOXED.get_or_init(|| Path::new("/.flatpak-info").exists())
}

/// Where the sandbox can read one of the computer's files.
pub fn inside(path: &Path) -> PathBuf {
    if !sandboxed() {
        return path.to_path_buf();
    }
    let mut parts = path.components();
    let (Some(std::path::Component::RootDir), Some(std::path::Component::Normal(first))) =
        (parts.next(), parts.next())
    else {
        return path.to_path_buf();
    };
    if SYSTEM.iter().any(|name| OsStr::new(name) == first) {
        Path::new(HOST).join(path.strip_prefix("/").unwrap_or(path))
    } else {
        path.to_path_buf()
    }
}

/// The path the computer knows a file by: /run/host/usr/share is /usr/share.
pub fn outside(path: &Path) -> PathBuf {
    if !sandboxed() {
        return path.to_path_buf();
    }
    match path.strip_prefix(HOST) {
        Ok(rest)
            if rest.components().next().is_some_and(|first| {
                SYSTEM
                    .iter()
                    .any(|name| OsStr::new(name) == first.as_os_str())
            }) =>
        {
            Path::new("/").join(rest)
        }
        _ => path.to_path_buf(),
    }
}

/// A folder or file the sandbox can read, for one the computer knows.
pub fn inside_file(file: &gio::File) -> gio::File {
    match file.path() {
        Some(path) if sandboxed() => {
            let mapped = inside(&path);
            if mapped == path {
                file.clone()
            } else {
                gio::File::for_path(mapped)
            }
        }
        _ => file.clone(),
    }
}

/// A file's parent as the computer sees it: /run/host/usr's is /.
pub fn parent(file: &gio::File) -> Option<gio::File> {
    if sandboxed()
        && let Some(path) = file.path()
        && outside(&path) != path
        && outside(&path).parent() == Some(Path::new("/"))
    {
        return Some(gio::File::for_path("/"));
    }
    file.parent()
}

/// An argument for one of the computer's programs: a /run/host path made
/// the computer's again; anything else as it is.
fn host_arg(arg: &OsStr) -> OsString {
    if arg.as_bytes().starts_with(HOST.as_bytes()) {
        outside(Path::new(arg)).into_os_string()
    } else {
        arg.to_os_string()
    }
}

/// One of the computer's programs, built the way std::process::Command is.
pub struct HostCommand {
    program: OsString,
    args: Vec<OsString>,
    folder: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
}

pub fn command(program: impl AsRef<OsStr>) -> HostCommand {
    HostCommand {
        program: program.as_ref().to_os_string(),
        args: Vec::new(),
        folder: None,
        env: Vec::new(),
    }
}

impl HostCommand {
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_os_string()));
        self
    }

    pub fn current_dir(&mut self, folder: impl AsRef<Path>) -> &mut Self {
        self.folder = Some(folder.as_ref().to_path_buf());
        self
    }

    /// The std::process::Command to run: the program itself, or in the
    /// Flatpak, flatpak-spawn running it on the computer.
    pub fn std(&self) -> Command {
        let mut words = vec![self.program.clone()];
        words.extend(self.args.iter().cloned());
        let words = argv(words, self.folder.as_deref(), &self.env);
        let mut command = Command::new(&words[0]);
        command.args(&words[1..]);
        if !sandboxed() {
            if let Some(folder) = &self.folder {
                command.current_dir(folder);
            }
            command.envs(self.env.iter().map(|(k, v)| (k, v)));
        }
        command
    }

    pub fn spawn(&self) -> std::io::Result<std::process::Child> {
        self.std().spawn()
    }
}

/// A command line for gio::Subprocess, in this folder with these variables
/// in the Flatpak (outside it, the caller sets those as before).
pub fn argv(
    words: Vec<OsString>,
    folder: Option<&Path>,
    env: &[(OsString, OsString)],
) -> Vec<OsString> {
    if !sandboxed() {
        return words;
    }
    // Not --watch-bus: what it starts (a terminal, an app) outlives File
    // Manager, as it does outside the Flatpak
    let mut host: Vec<OsString> = vec!["flatpak-spawn".into(), "--host".into()];
    if let Some(folder) = folder {
        let mut option = OsString::from("--directory=");
        option.push(outside(folder));
        host.push(option);
    }
    for (key, value) in env {
        let mut option = OsString::from("--env=");
        option.push(key);
        option.push("=");
        option.push(value);
        host.push(option);
    }
    host.extend(words.iter().map(|word| host_arg(word)));
    host
}

/// A program on the computer's PATH, by name.
pub fn find_program(name: &str) -> Option<PathBuf> {
    if !sandboxed() {
        return glib::find_program_in_path(name);
    }
    static FOUND: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);
    let mut found = FOUND
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    found
        .get_or_insert_with(HashMap::new)
        .entry(name.to_owned())
        .or_insert_with(|| {
            let output = Command::new("flatpak-spawn")
                .args(["--host", "sh", "-c", "command -v \"$0\"", name])
                .output()
                .ok()?;
            let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            (output.status.success() && path.starts_with('/')).then(|| PathBuf::from(path))
        })
        .clone()
}

/// In the Flatpak, the account's own shared folders, not the sandbox's:
/// the thumbnail cache, recent files and thumbnailers in ~/.cache and
/// ~/.local/share are the computer's, which other apps use too. GLib reads
/// these when it first needs them, so this comes first in main.
pub fn use_the_accounts_folders() {
    if !sandboxed() {
        return;
    }
    let home = glib::home_dir();
    // SAFETY: at the very start of main, before any other thread.
    unsafe {
        std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
        std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_flatpak_paths_stay() {
        if sandboxed() {
            return;
        }
        assert_eq!(inside(Path::new("/usr/share")), Path::new("/usr/share"));
        assert_eq!(
            outside(Path::new("/run/host/usr")),
            Path::new("/run/host/usr")
        );
        assert_eq!(
            argv(vec!["ls".into()], None, &[]),
            vec![OsString::from("ls")]
        );
    }
}
