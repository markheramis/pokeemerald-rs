use std::path::Path;

#[cfg(windows)]
use win_fs::WindowsFileIdentity;

/// Pins the directory used for payload writes. The claim starts at the
/// successful open, not at the directory's creation.
pub(super) struct StagedDirClaim {
    hold: Option<std::fs::File>,
}

impl StagedDirClaim {
    pub(super) fn require_path(&self, path: &Path) -> std::io::Result<()> {
        let hold = self.hold.as_ref().ok_or_else(|| unpinned_directory(path))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let identity = hold.metadata()?;
            let found = std::fs::symlink_metadata(path)?;
            if !found.is_dir() || (identity.dev(), identity.ino()) != (found.dev(), found.ino()) {
                return Err(std::io::Error::other(format!(
                    "the directory at {} no longer matches the capture's held directory",
                    path.display()
                )));
            }
        }
        #[cfg(not(unix))]
        let _ = hold;
        Ok(())
    }

    #[cfg(unix)]
    pub(super) fn write_payload(
        &self,
        staged: &Path,
        name: &str,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        use std::io::Write as _;
        let hold = self
            .hold
            .as_ref()
            .ok_or_else(|| unpinned_directory(staged))?;
        let fd = rustix::fs::openat(
            hold,
            name,
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
        super::staging::ensure_owner_read_write(&fd)?;
        std::fs::File::from(fd).write_all(bytes)
    }

    #[cfg(not(unix))]
    pub(super) fn write_payload(
        &self,
        staged: &Path,
        name: &str,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        use std::io::Write as _;
        self.require_path(staged)?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staged.join(name))?
            .write_all(bytes)
    }

    /// Fails unless `path`'s final component names the held directory inside
    /// `output`'s held directory. The lookup goes through that directory's
    /// handle, not the pathname, so a generation carried into whatever the
    /// output pathname names now cannot pass for one in the held directory.
    #[cfg(unix)]
    pub(super) fn require_entry_in(
        &self,
        output: &OutputDirClaim,
        path: &Path,
    ) -> std::io::Result<()> {
        let hold = self.hold.as_ref().ok_or_else(|| unpinned_directory(path))?;
        let identity = rustix::fs::fstat(hold)?;
        let found = rustix::fs::statat(
            &output.hold,
            entry_name(path)?,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )?;
        if rustix::fs::FileType::from_raw_mode(found.st_mode) != rustix::fs::FileType::Directory
            || (identity.st_dev, identity.st_ino) != (found.st_dev, found.st_ino)
        {
            return Err(std::io::Error::other(format!(
                "the directory at {} no longer matches the capture's held directory",
                path.display()
            )));
        }
        Ok(())
    }

    /// Windows resolves the pathname; see [`OutputDirClaim`] for the residual.
    #[cfg(not(unix))]
    pub(super) fn require_entry_in(
        &self,
        _output: &OutputDirClaim,
        path: &Path,
    ) -> std::io::Result<()> {
        self.require_path(path)
    }

    // Unix carries the handle through rename. Windows must release its sharing
    // restriction first; preserving identity across that gap is issue #1345.
    #[cfg_attr(
        not(windows),
        expect(
            clippy::unused_self,
            reason = "only Windows releases its hold before rename"
        )
    )]
    pub(super) fn release_hold(&mut self) {
        #[cfg(windows)]
        self.hold.take();
    }
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

fn unpinned_directory(path: &Path) -> std::io::Error {
    std::io::Error::other(format!(
        "no handle pins the capture directory at {}; publication was refused",
        path.display()
    ))
}

/// `O_SEARCH` on Apple targets, which have no `O_PATH`: search access to a
/// directory without the read access `O_RDONLY` demands. Apple spells it
/// `O_EXEC | O_DIRECTORY` (`libc`'s `O_EXEC`, `0x4000_0000`); `rustix` 1.1 has
/// no flag for it, so the value travels as externally defined `OFlags` bits.
/// Callers always add `DIRECTORY`.
#[cfg(target_vendor = "apple")]
const APPLE_SEARCH: rustix::fs::OFlags = rustix::fs::OFlags::from_bits_retain(0x4000_0000);

#[cfg(all(unix, test))]
fn open_held_dir(path: &Path, access: rustix::fs::OFlags) -> std::io::Result<std::fs::File> {
    rustix::fs::open(
        path,
        access | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map(std::fs::File::from)
    .map_err(Into::into)
}

/// Pins a staging directory with the strongest access the platform allows.
/// `open` resolves the directory, by pathname or through a handle.
#[cfg(unix)]
fn claim_staged_dir_with(
    open: impl Fn(rustix::fs::OFlags) -> std::io::Result<std::fs::File>,
) -> std::io::Result<StagedDirClaim> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let access = rustix::fs::OFlags::PATH;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let access = rustix::fs::OFlags::RDONLY;
    let held = open(access);
    // A `0444` umask leaves the staging directory at mode `0333`, so `O_RDONLY`
    // is refused. The hold only needs search access (`openat`, `fstat`).
    #[cfg(target_vendor = "apple")]
    let held = match held {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => open(APPLE_SEARCH),
        held => held,
    };
    // A claim without a hold could never write a payload, so fail instead.
    Ok(StagedDirClaim { hold: Some(held?) })
}

/// Pathname-based claim; production claims through [`OutputDirClaim::claim_staged_dir`].
#[cfg(all(unix, test))]
pub(super) fn claim_staged_dir(path: &Path) -> std::io::Result<StagedDirClaim> {
    let claim = claim_staged_dir_with(|access| open_held_dir(path, access))?;
    claim.require_path(path)?;
    Ok(claim)
}

#[cfg(windows)]
pub(super) fn claim_staged_dir(path: &Path) -> std::io::Result<StagedDirClaim> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let hold = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(0)
        .open(path)?;
    let found = hold.metadata()?;
    if !found.is_dir() || found.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::other("the capture directory was replaced"));
    }
    Ok(StagedDirClaim { hold: Some(hold) })
}

#[cfg(not(any(unix, windows)))]
pub(super) fn claim_staged_dir(_path: &Path) -> std::io::Result<StagedDirClaim> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(windows)]
pub(super) fn claim_promoted_dir(path: &Path) -> std::io::Result<StagedDirClaim> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        match claim_staged_dir(path) {
            // Concurrent directory operations can briefly deny the exclusive
            // open without replacing the directory. Other failures are final.
            Err(error)
                if matches!(error.raw_os_error(), Some(32 | 33))
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now();
            }
            result => return result,
        }
    }
}

/// Pins the output directory for a whole publication.
///
/// On Unix the generation is promoted ([`Self::promote_without_replacement`])
/// and verified (`StagedDirClaim::require_entry_in`), and the pointer is staged
/// and promoted ([`Self::stage_pointer`], `staging::StagedFile`), all relative
/// to this held directory. The pointer therefore lands beside the generation it
/// names, whatever the output pathname names by then.
///
/// Windows has no handle-relative rename through the approved surface, so the
/// pathname stays in the loop: the hold keeps the directory from being renamed
/// away and [`Self::require_path`] compares a fresh open of the path with the
/// held handle, but a swap between a check and a rename remains possible
/// (check-then-act).
pub(super) struct OutputDirClaim {
    hold: std::fs::File,
}

impl OutputDirClaim {
    /// Fails unless `path` still resolves to the held directory.
    pub(super) fn require_path(&self, path: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let identity = self.hold.metadata()?;
            let found = std::fs::metadata(path)?;
            if !found.is_dir() || (identity.dev(), identity.ino()) != (found.dev(), found.ino()) {
                return Err(no_longer_the_held_directory(path));
            }
        }
        #[cfg(windows)]
        {
            let fresh = open_output_dir(path)?;
            if !fresh.metadata()?.is_dir()
                || WindowsFileIdentity::of(&fresh)? != WindowsFileIdentity::of(&self.hold)?
            {
                return Err(no_longer_the_held_directory(path));
            }
        }
        #[cfg(not(any(unix, windows)))]
        let _ = path;
        Ok(())
    }

    /// Creates `path`'s final component as a directory inside the held
    /// directory, so a replaced output pathname cannot receive it.
    #[cfg(unix)]
    pub(super) fn create_staged_dir(&self, path: &Path) -> std::io::Result<()> {
        rustix::fs::mkdirat(
            &self.hold,
            entry_name(path)?,
            // `create_dir`'s own 0o777; the process umask narrows it.
            rustix::fs::Mode::RWXU | rustix::fs::Mode::RWXG | rustix::fs::Mode::RWXO,
        )
        .map_err(Into::into)
    }

    #[cfg(not(unix))]
    #[expect(
        clippy::unused_self,
        reason = "one signature for every platform; only the unix arm creates through the hold"
    )]
    pub(super) fn create_staged_dir(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir(path)
    }

    /// Pins `path`'s final component inside the held directory, then verifies
    /// it through the held directory's handle.
    #[cfg(unix)]
    pub(super) fn claim_staged_dir(&self, path: &Path) -> std::io::Result<StagedDirClaim> {
        let name = entry_name(path)?;
        let claim = claim_staged_dir_with(|access| {
            rustix::fs::openat(
                &self.hold,
                name,
                access | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::empty(),
            )
            .map(std::fs::File::from)
            .map_err(Into::into)
        })?;
        claim.require_entry_in(self, path)?;
        Ok(claim)
    }

    #[cfg(not(unix))]
    #[expect(
        clippy::unused_self,
        reason = "one signature for every platform; only the unix arm claims through the hold"
    )]
    pub(super) fn claim_staged_dir(&self, path: &Path) -> std::io::Result<StagedDirClaim> {
        claim_staged_dir(path)
    }

    /// Renames `staged` to `generation` (final components, inside the held
    /// directory) with `RENAME_NOREPLACE` semantics: anything at `generation`
    /// is refused. A `staged` moved out of the held directory is not found.
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    pub(super) fn promote_without_replacement(
        &self,
        staged: &Path,
        generation: &Path,
    ) -> std::io::Result<()> {
        rustix::fs::renameat_with(
            &self.hold,
            entry_name(staged)?,
            &self.hold,
            entry_name(generation)?,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(Into::into)
    }

    /// Windows has no handle-relative rename through the approved surface, so
    /// promotion resolves the pathname (see the residual above).
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    #[expect(
        clippy::unused_self,
        reason = "one signature for every platform; only the unix arms rename through the hold"
    )]
    pub(super) fn promote_without_replacement(
        &self,
        staged: &Path,
        generation: &Path,
    ) -> std::io::Result<()> {
        rename_without_replacement(staged, generation)
    }

    /// Stages a pointer at `path`'s final component inside the held directory.
    #[cfg(unix)]
    pub(super) fn stage_pointer(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> std::io::Result<super::staging::StagedFile> {
        super::staging::stage_in(&self.hold, path, bytes)
    }

    #[cfg(not(unix))]
    #[expect(
        clippy::unused_self,
        reason = "one signature for every platform; only the unix arm stages through the hold"
    )]
    pub(super) fn stage_pointer(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> std::io::Result<super::staging::StagedFile> {
        super::staging::stage(path, bytes)
    }
}

fn no_longer_the_held_directory(path: &Path) -> std::io::Error {
    std::io::Error::other(format!(
        "the output directory at {} no longer matches the capture's held directory",
        path.display()
    ))
}

/// Unlike the staging claim this follows a symlinked `output_dir`, which
/// callers may pass.
#[cfg(unix)]
pub(super) fn claim_output_dir(path: &Path) -> std::io::Result<OutputDirClaim> {
    // `O_PATH` descriptors work as the directory argument of `openat`,
    // `fstatat`, and `renameat`, so no read access is needed.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let access = rustix::fs::OFlags::PATH;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let access = rustix::fs::OFlags::RDONLY;
    let open = |access| {
        rustix::fs::open(
            path,
            access | rustix::fs::OFlags::DIRECTORY,
            rustix::fs::Mode::empty(),
        )
        .map(std::fs::File::from)
        .map_err(std::io::Error::from)
    };
    let hold = open(access);
    // A search-only output directory (mode `0333`) refuses `O_RDONLY`; see
    // `claim_staged_dir_with`.
    #[cfg(target_vendor = "apple")]
    let hold = match hold {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => open(APPLE_SEARCH),
        hold => hold,
    };
    Ok(OutputDirClaim { hold: hold? })
}

/// Opens `path` following any symlink or junction, without requesting access
/// and without `FILE_SHARE_DELETE`, so an open handle keeps the directory
/// from being renamed or deleted.
#[cfg(windows)]
fn open_output_dir(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_WRITE: u32 = 2;
    std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)
}

#[cfg(windows)]
pub(super) fn claim_output_dir(path: &Path) -> std::io::Result<OutputDirClaim> {
    open_output_dir(path).map(|hold| OutputDirClaim { hold })
}

#[cfg(not(any(unix, windows)))]
pub(super) fn claim_output_dir(_path: &Path) -> std::io::Result<OutputDirClaim> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(windows)]
fn rename_without_replacement(source: &Path, destination: &Path) -> std::io::Result<()> {
    // Windows rename can replace empty directories, so refuse explicitly. The
    // check is not atomic with the rename (#1345).
    match std::fs::symlink_metadata(destination) {
        Ok(_) => return Err(std::io::ErrorKind::AlreadyExists.into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::rename(source, destination)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
fn rename_without_replacement(_source: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic directory promotion without replacement is unavailable on this target",
    ))
}
