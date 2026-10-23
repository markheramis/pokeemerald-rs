//! Atomic publication of a pointer file. Unix stages through a held directory
//! (`stage_in`), so lookups and the promoting rename are relative to it; other
//! platforms stage by path (`stage`).
//!
//! The staged file is created exclusively and kept open. [`StagedFile::publish`]
//! checks that the staging name still resolves to that open file before
//! renaming, so a symlink or replacement planted at the name is refused. On
//! Unix the handle survives the rename, so the promoted entry is checked too:
//! a replacement that wins the gap between check and rename lands on the
//! pointer path but is never reported as published.
//!
//! Failures never delete. A held file's identity check cannot make a separate
//! pathname deletion conditional on that identity, so the staged or unverified
//! promoted entry is retained and its last known path reported.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Opens `path` for writing, failing if anything (file, directory, or symlink)
/// already holds that name.
///
/// On Windows the handle shares nothing, so until it is dropped no other opener
/// can open, delete, or rename the entry.
#[cfg(any(not(unix), test))]
fn create_new_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;

        options.share_mode(0);
    }
    options.open(path)
}

/// Creates `path` exclusively, writes and syncs `bytes`, and returns the held
/// staging file. Fails with `AlreadyExists` if `path` already names anything,
/// including a planted symlink. A failure after the create retains the file.
#[cfg(any(not(unix), test))]
pub(super) fn stage(path: &Path, bytes: &[u8]) -> std::io::Result<StagedFile> {
    stage_file(create_new_exclusive(path)?, path, bytes)
}

/// Stages `path`'s final component inside the held directory `dir`, so the
/// entry can only appear there. The staged file publishes relative to `dir` too.
#[cfg(unix)]
pub(super) fn stage_in(
    dir: &std::fs::File,
    path: &Path,
    bytes: &[u8],
) -> std::io::Result<StagedFile> {
    stage_in_with(dir, path, bytes, std::fs::File::try_clone)
}

/// [`stage_in`] with the directory handle's duplication supplied, so tests can
/// fail it.
#[cfg(unix)]
pub(super) fn stage_in_with(
    dir: &std::fs::File,
    path: &Path,
    bytes: &[u8],
    clone_dir: impl FnOnce(&std::fs::File) -> std::io::Result<std::fs::File>,
) -> std::io::Result<StagedFile> {
    // Duplicated before the create, so a failure here leaves nothing behind.
    let dir = clone_dir(dir)?;
    let fd = rustix::fs::openat(
        &dir,
        entry_name(path)?,
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW,
        // `File::create`'s own 0o666; the process umask narrows it.
        rustix::fs::Mode::RUSR
            | rustix::fs::Mode::WUSR
            | rustix::fs::Mode::RGRP
            | rustix::fs::Mode::WGRP
            | rustix::fs::Mode::ROTH
            | rustix::fs::Mode::WOTH,
    )?;
    let mut staged = stage_file(std::fs::File::from(fd), path, bytes);
    if let Ok(staged) = &mut staged {
        staged.dir = Some(dir);
    }
    staged
}

/// Puts owner read and write back on `file` after a `0o666` create.
///
/// The process umask can strip those bits. Group and other stay as that
/// umask left them.
#[cfg(unix)]
pub(super) fn ensure_owner_read_write(file: impl rustix::fd::AsFd) -> std::io::Result<()> {
    let mode = rustix::fs::Mode::from_raw_mode(rustix::fs::fstat(&file)?.st_mode);
    rustix::fs::fchmod(file, mode | rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR)
        .map_err(Into::into)
}

#[cfg(unix)]
fn entry_name(path: &Path) -> std::io::Result<&std::ffi::OsStr> {
    path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} names no directory entry", path.display()),
        )
    })
}

fn stage_file(file: std::fs::File, path: &Path, bytes: &[u8]) -> std::io::Result<StagedFile> {
    let result = (|| {
        #[cfg(unix)]
        ensure_owner_read_write(&file)?;
        let mut writer = std::io::BufWriter::new(&file);
        writer.write_all(bytes)?;
        writer.flush()?;
        file.sync_all()
    })();
    #[cfg(not(windows))]
    let hold = file;
    #[cfg(windows)]
    let hold = Some(file);
    let staged = StagedFile {
        path: path.to_path_buf(),
        hold,
        #[cfg(unix)]
        dir: None,
    };
    match result {
        Ok(()) => Ok(staged),
        Err(source) => Err(staged.report_retained(&source)),
    }
}

/// The handle that wrote the staged pointer. While it lives the file cannot be
/// freed, so nothing that takes its name can inherit its identity. A rename is
/// indifferent to it, so it stays open until publication or abandonment.
#[cfg(not(windows))]
type Hold = std::fs::File;

/// As [`Hold`] above, but `None` once [`StagedFile::release_hold`] gives it up:
/// Windows refuses to rename a file whose handle shares nothing.
#[cfg(windows)]
type Hold = Option<std::fs::File>;

/// Keeps `hold` for the life of the [`StagedFile`] (see [`Hold`]).
#[cfg(not(windows))]
fn release(_hold: &mut Hold) {}

/// Drops `hold` so the staging name can be renamed (see [`Hold`]).
#[cfg(windows)]
fn release(hold: &mut Hold) {
    drop(hold.take());
}

/// Whether `found` is the file `hold` holds open: same device and inode.
#[cfg(unix)]
fn is_the_held_file(hold: &Hold, found: &std::fs::Metadata) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;

    let staged = hold.metadata()?;
    Ok((staged.dev(), staged.ino()) == (found.dev(), found.ino()))
}

/// Whether `found` is the file `hold` holds open. Windows reads back no
/// identity, so while the hold lives (see [`create_new_exclusive`]) the name
/// cannot mean another file. Once released the identity is unknowable, and
/// that is reported rather than guessed.
#[cfg(windows)]
fn is_the_held_file(hold: &Hold, _found: &std::fs::Metadata) -> std::io::Result<bool> {
    if hold.is_some() {
        Ok(true)
    } else {
        Err(std::io::Error::other(
            "the staging hold was released, so the file's identity can no longer be confirmed",
        ))
    }
}

/// Whether `found` is the file `hold` holds open. Without inodes or a sharing
/// hold, the caller's regular-file test is all there is.
#[cfg(not(any(unix, windows)))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "one signature for every platform; only the unix and windows arms can fail to read an identity"
)]
fn is_the_held_file(_hold: &Hold, _found: &std::fs::Metadata) -> std::io::Result<bool> {
    Ok(true)
}

/// A staged pointer file and the hold that keeps its name its own.
pub(super) struct StagedFile {
    path: PathBuf,
    hold: Hold,
    /// The held directory this file was staged in, when it was staged through
    /// one: every later lookup and the promoting rename then name entries
    /// relative to it, never through a pathname.
    #[cfg(unix)]
    dir: Option<std::fs::File>,
}

impl StagedFile {
    /// Whether `path` names this staged file rather than a symlink, directory,
    /// or other entry that took its name. A failed lookup is an error, not
    /// "replaced".
    fn matches(&self, path: &Path) -> std::io::Result<bool> {
        #[cfg(unix)]
        if let Some(dir) = &self.dir {
            let found = rustix::fs::statat(
                dir,
                entry_name(path)?,
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            )?;
            let staged = rustix::fs::fstat(&self.hold)?;
            return Ok(rustix::fs::FileType::from_raw_mode(found.st_mode)
                == rustix::fs::FileType::RegularFile
                && (staged.st_dev, staged.st_ino) == (found.st_dev, found.st_ino));
        }
        let found = std::fs::symlink_metadata(path)?;
        Ok(found.file_type().is_file() && is_the_held_file(&self.hold, &found)?)
    }

    /// Whether the staging path still names this staged file.
    fn still_ours(&self) -> std::io::Result<bool> {
        self.matches(&self.path)
    }

    /// Gives up the hold so the promoting rename can take the staging name.
    fn release_hold(&mut self) {
        release(&mut self.hold);
    }

    /// Folds a stage or publish failure into `source` and reports the staged
    /// file's last known path. The file is left for an operator to inspect and
    /// remove (see the module docs).
    fn report_retained(&self, source: &std::io::Error) -> std::io::Error {
        std::io::Error::new(
            source.kind(),
            format!(
                "{source}; the staging file was not removed; last known path: {}",
                self.path.display()
            ),
        )
    }

    /// Abandons publication after a caller-found failure, retaining and
    /// reporting the file as a failed publish would.
    pub(super) fn retain(self, source: &std::io::Error) -> std::io::Error {
        self.report_retained(source)
    }

    /// Folds a post-rename identity failure into `source` and reports the
    /// promoted pointer's path. The entry is left for an operator to inspect
    /// and remove (see the module docs).
    #[cfg(unix)]
    fn report_unverified(dest: &Path, source: &std::io::Error) -> std::io::Error {
        std::io::Error::new(
            source.kind(),
            format!(
                "{source}; the promoted pointer was not removed; last known path: {}",
                dest.display()
            ),
        )
    }

    /// Renames the staged file onto `dest`, relative to the held directory when
    /// there is one.
    fn rename_to(&self, dest: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(dir) = &self.dir {
            return rustix::fs::renameat(dir, entry_name(&self.path)?, dir, entry_name(dest)?)
                .map_err(Into::into);
        }
        std::fs::rename(&self.path, dest)
    }

    /// Renames the staged file to `dest` after confirming the staging name
    /// still is this file; a replaced or unverifiable name is refused.
    pub(super) fn publish(self, dest: &Path) -> std::io::Result<()> {
        self.publish_with(dest, || {}, || {})
    }

    /// [`Self::publish`] with test seams: `before_rename` runs just before the
    /// promoting rename, and `on_rename_failure` right after a failed one.
    /// Production passes no-ops; tests use them to plant a replacement at the
    /// staging name.
    pub(super) fn publish_with(
        mut self,
        dest: &Path,
        before_rename: impl FnOnce(),
        on_rename_failure: impl FnOnce(),
    ) -> std::io::Result<()> {
        match self.still_ours() {
            Ok(true) => {}
            Ok(false) => {
                return Err(self.report_retained(&std::io::Error::other(format!(
                    "staging file {} was replaced before it could be published",
                    self.path.display()
                ))));
            }
            Err(error) => {
                let wrapped = std::io::Error::new(
                    error.kind(),
                    format!(
                        "ownership of the staging file {} could not be confirmed before publishing: {error}",
                        self.path.display()
                    ),
                );
                return Err(self.report_retained(&wrapped));
            }
        }
        self.release_hold();
        before_rename();
        self.rename_to(dest).map_err(|error| {
            on_rename_failure();
            self.report_retained(&error)
        })?;
        #[cfg(unix)]
        match self.matches(dest) {
            Ok(true) => {}
            Ok(false) => {
                return Err(Self::report_unverified(
                    dest,
                    &std::io::Error::other(format!(
                        "the promoted pointer {} does not match the staged file",
                        dest.display()
                    )),
                ));
            }
            Err(error) => {
                return Err(Self::report_unverified(
                    dest,
                    &std::io::Error::new(
                        error.kind(),
                        format!(
                            "the promoted pointer {} could not be confirmed after publishing: {error}",
                            dest.display()
                        ),
                    ),
                ));
            }
        }
        Ok(())
    }
}
