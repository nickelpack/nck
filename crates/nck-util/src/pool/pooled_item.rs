use std::ops::{Deref, DerefMut};

use super::PoolReturn;

/// A value that was retrieved from a `Pool`.
pub struct PooledItem<'a, T> {
    value: Option<T>,
    pool: &'a dyn PoolReturn<T>,
}

impl<'a, T> PooledItem<'a, T> {
    /// Creates a new pooled item.
    pub fn new(value: T, pool: &'a impl PoolReturn<T>) -> Self {
        Self {
            value: Some(value),
            pool,
        }
    }

    /// Gets a reference to the value.
    ///
    #[inline]
    pub fn get(&'a self) -> &'a T {
        self.value.as_ref().unwrap()
    }

    /// Gets a mutable reference to the value.
    ///
    #[inline]
    pub fn get_mut(&'a mut self) -> &'a mut T {
        self.value.as_mut().unwrap()
    }

    /// Forgets the contained value
    ///
    /// Prevents the contained value from being returned to the pool, and returns it to the caller.
    pub fn forget(mut self) -> T {
        self.value.take().unwrap()
    }
}

impl<'a, T: std::fmt::Debug> std::fmt::Debug for PooledItem<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.value.as_ref() {
            Some(v) => v.fmt(f),
            None => f.debug_tuple("EmptyPooledItem").finish(),
        }
    }
}

impl<'a, T> Deref for PooledItem<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.value.as_ref().unwrap()
    }
}

impl<'a, T> DerefMut for PooledItem<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value.as_mut().unwrap()
    }
}

impl<'a, T> AsRef<T> for PooledItem<'a, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self.value.as_ref().unwrap()
    }
}

impl<'a, T> AsMut<T> for PooledItem<'a, T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self.value.as_mut().unwrap()
    }
}

impl<'a, T> Drop for PooledItem<'a, T> {
    #[inline]
    fn drop(&mut self) {
        if let Some(old) = self.value.take() {
            self.pool.return_value(old)
        }
    }
}
