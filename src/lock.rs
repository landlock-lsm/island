// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::IslandError;
use std::{fs, io, marker::PhantomData};
use thiserror::Error;

/// Guard that holds a lock on a profile directory.
///
/// This should only be used against a profile directory, not a subdirectory.
pub(crate) struct ProfileGuard<'a, L>(&'a fs::File, PhantomData<L>)
where
    L: ProfileLock;

impl<'a, L> ProfileGuard<'a, L>
where
    L: ProfileLock,
{
    pub fn new(file: &'a fs::File) -> io::Result<Self> {
        L::lock(file)?;
        Ok(ProfileGuard(file, PhantomData))
    }

    pub fn modify<F, T>(&self, f: F) -> Result<T, L::Error>
    where
        F: FnOnce() -> Result<T, L::Error>,
    {
        L::modify(f)
    }
}

impl<'a, L> Drop for ProfileGuard<'a, L>
where
    L: ProfileLock,
{
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(crate) trait ProfileLock {
    type Error: From<io::Error>;

    fn lock(file: &fs::File) -> io::Result<()>;

    fn modify<F, T>(f: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> Result<T, Self::Error>;
}

pub(crate) struct SharedLock;

#[derive(Debug, Error)]
pub(crate) enum SharedLockError {
    #[error(transparent)]
    Island(#[from] IslandError),
    #[error("profile needs update")]
    NeedsUpdate,
}

impl From<io::Error> for SharedLockError {
    fn from(e: io::Error) -> Self {
        Self::Island(e.into())
    }
}

impl ProfileLock for SharedLock {
    type Error = SharedLockError;

    fn lock(file: &fs::File) -> io::Result<()> {
        file.lock_shared()
    }

    fn modify<F, T>(_: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> Result<T, Self::Error>,
    {
        Err(SharedLockError::NeedsUpdate)
    }
}

pub(crate) struct ExclusiveLock;

impl ProfileLock for ExclusiveLock {
    type Error = IslandError;

    fn lock(file: &fs::File) -> io::Result<()> {
        file.lock()
    }

    fn modify<F, T>(f: F) -> Result<T, Self::Error>
    where
        F: FnOnce() -> Result<T, Self::Error>,
    {
        f()
    }
}
