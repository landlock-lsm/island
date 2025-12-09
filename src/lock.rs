// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::IslandError;
use std::{
    fs::File,
    io,
    marker::PhantomData,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Guard that holds a lock on a profile directory.
///
/// This should be used to handle workspaces and Landlock configuration, which
/// might be modified by Island.  For now, reading a profile.toml file does not
/// require such guard.
pub(crate) struct ProfileGuard<L>
where
    L: ProfileLock,
{
    profile_dir: File,
    profile_path: PathBuf,
    lock: PhantomData<L>,
}

impl<L> ProfileGuard<L>
where
    L: ProfileLock,
{
    /// profile_path should only a profile directory, not a subdirectory nor
    /// another file.
    pub fn new(profile_path: PathBuf) -> io::Result<Self> {
        let profile_dir = File::open(&profile_path)?;
        L::lock(&profile_dir)?;
        Ok(ProfileGuard {
            profile_dir,
            profile_path,
            lock: PhantomData,
        })
    }

    pub fn modify<F, T>(&self, f: F) -> Result<T, L::Error>
    where
        F: FnOnce() -> Result<T, L::Error>,
    {
        L::modify(f)
    }

    pub fn path(&self) -> &Path {
        &self.profile_path
    }

    pub fn path_landlock(&self) -> PathBuf {
        self.profile_path.join("landlock")
    }
}

impl<L> Drop for ProfileGuard<L>
where
    L: ProfileLock,
{
    fn drop(&mut self) {
        let _ = self.profile_dir.unlock();
    }
}

pub(crate) trait ProfileLock {
    type Error;

    fn lock(file: &File) -> io::Result<()>;

    fn modify<F, T>(f: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> Result<T, Self::Error>;
}

pub(crate) struct SharedLock<E>(PhantomData<E>);

#[derive(Debug, Error)]
pub(crate) enum SharedLockError<E> {
    #[error(transparent)]
    Inner(#[from] E),
    #[error("profile needs update")]
    NeedsUpdate,
}

impl From<io::Error> for SharedLockError<IslandError> {
    fn from(e: io::Error) -> Self {
        Self::Inner(e.into())
    }
}

impl<E> ProfileLock for SharedLock<E> {
    type Error = SharedLockError<E>;

    fn lock(file: &File) -> io::Result<()> {
        file.lock_shared()
    }

    fn modify<F, T>(_: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> Result<T, Self::Error>,
    {
        Err(SharedLockError::NeedsUpdate)
    }
}

pub(crate) struct ExclusiveLock<E>(PhantomData<E>);

impl<E> ProfileLock for ExclusiveLock<E> {
    type Error = E;

    fn lock(file: &File) -> io::Result<()> {
        file.lock()
    }

    fn modify<F, T>(f: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> Result<T, Self::Error>,
    {
        f()
    }
}
